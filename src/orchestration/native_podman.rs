//! Native Podman container runtime for Linux
//!
//! Implements the ContainerRuntime trait using direct Podman execution
//! without a VM layer. Requires rootless Podman to be properly configured.

use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::podman::ContainerConfig;
use crate::orchestration::runtime::ContainerRuntime;
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info};

/// Container runtime using native rootless Podman (for Linux)
pub struct NativePodmanRuntime;

impl NativePodmanRuntime {
    /// Create a new native Podman runtime
    pub fn new() -> Self {
        Self
    }

    /// Check if Podman is installed
    async fn podman_installed() -> bool {
        Command::new("podman")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Check if rootless Podman is properly configured
    async fn rootless_configured() -> MinotaurResult<bool> {
        // Check if user namespaces are available
        let output = Command::new("podman")
            .args(["info", "--format", "{{.Host.Security.Rootless}}"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("podman info", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim() == "true")
    }

    /// Execute a Podman command and return the output
    async fn exec(&self, args: &[&str]) -> MinotaurResult<std::process::Output> {
        debug!("Executing: podman {:?}", args);

        Command::new("podman")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed(format!("podman {:?}", args), e))
    }

    /// Execute a Podman command interactively
    async fn exec_interactive(&self, args: &[&str]) -> MinotaurResult<i32> {
        debug!("Executing interactively: podman {:?}", args);

        let status = Command::new("podman")
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .map_err(|e| MinotaurError::command_failed(format!("podman {:?}", args), e))?;

        Ok(status.code().unwrap_or(-1))
    }

    /// Pull an image
    async fn pull(&self, image: &str) -> MinotaurResult<()> {
        info!("Pulling image: {}", image);

        let output = self.exec(&["pull", image]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::ImagePull {
                image: image.to_string(),
                reason: stderr.to_string(),
            })
        }
    }

    /// Check if image exists locally
    async fn image_exists(&self, image: &str) -> MinotaurResult<bool> {
        let output = self.exec(&["image", "exists", image]).await?;
        Ok(output.status.success())
    }
}

impl Default for NativePodmanRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContainerRuntime for NativePodmanRuntime {
    async fn is_available(&self) -> MinotaurResult<bool> {
        if !Self::podman_installed().await {
            return Ok(false);
        }
        Self::rootless_configured().await
    }

    async fn ensure_ready(&self) -> MinotaurResult<()> {
        if !Self::podman_installed().await {
            return Err(MinotaurError::PodmanNotFound);
        }

        if !Self::rootless_configured().await? {
            return Err(MinotaurError::PodmanRootlessSetup {
                reason: "Rootless Podman not configured. Run: podman system migrate".to_string(),
            });
        }

        Ok(())
    }

    async fn run(&self, config: &ContainerConfig, command: &[String]) -> MinotaurResult<String> {
        // Ensure image is available
        if !self.image_exists(&config.image).await? {
            self.pull(&config.image).await?;
        }

        let mut args = vec!["run".to_string()];

        // Detach mode
        args.push("-d".to_string());

        // Interactive/TTY
        if config.interactive {
            args.push("-i".to_string());
        }
        if config.tty {
            args.push("-t".to_string());
        }

        // Working directory
        args.push("-w".to_string());
        args.push(config.workdir.clone());

        // Network
        args.push("--network".to_string());
        args.push(config.network.clone());

        // Volumes
        for v in &config.volumes {
            args.push("-v".to_string());
            args.push(v.clone());
        }

        // Environment variables
        for (k, v) in &config.env {
            args.push("-e".to_string());
            args.push(format!("{}={}", k, v));
        }

        // Image
        args.push(config.image.clone());

        // Command to run
        args.extend(command.iter().cloned());

        debug!("Running container: podman {:?}", args);

        let args_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.exec(&args_refs).await?;

        if output.status.success() {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info!(
                "Container started: {}",
                &container_id[..12.min(container_id.len())]
            );
            Ok(container_id)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::ContainerStart(stderr.to_string()))
        }
    }

    async fn attach(&self, container_id: &str) -> MinotaurResult<i32> {
        debug!("Attaching to container: {}", container_id);
        self.exec_interactive(&["attach", container_id]).await
    }

    async fn stop(&self, container_id: &str) -> MinotaurResult<()> {
        debug!("Stopping container: {}", container_id);

        let output = self.exec(&["stop", container_id]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::command_exec("podman stop", stderr))
        }
    }

    async fn kill(&self, container_id: &str) -> MinotaurResult<()> {
        debug!("Killing container: {}", container_id);

        let output = self.exec(&["kill", container_id]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::command_exec("podman kill", stderr))
        }
    }

    async fn remove(&self, container_id: &str) -> MinotaurResult<()> {
        debug!("Removing container: {}", container_id);

        let output = self.exec(&["rm", "-f", container_id]).await?;

        if output.status.success() {
            Ok(())
        } else {
            // Ignore error if container doesn't exist
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no such container") {
                Ok(())
            } else {
                Err(MinotaurError::command_exec("podman rm", stderr))
            }
        }
    }

    async fn logs(&self, container_id: &str, lines: u32) -> MinotaurResult<String> {
        let tail_arg = if lines == 0 {
            "all".to_string()
        } else {
            lines.to_string()
        };

        let output = self.exec(&["logs", "--tail", &tail_arg, container_id]).await?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn logs_follow(&self, container_id: &str) -> MinotaurResult<()> {
        self.exec_interactive(&["logs", "-f", container_id]).await?;
        Ok(())
    }

    fn runtime_name(&self) -> &'static str {
        "Native Podman"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_podman_runtime_new() {
        let runtime = NativePodmanRuntime::new();
        assert_eq!(runtime.runtime_name(), "Native Podman");
    }

    #[test]
    fn native_podman_runtime_default() {
        let runtime = NativePodmanRuntime::default();
        assert_eq!(runtime.runtime_name(), "Native Podman");
    }
}
