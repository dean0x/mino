//! Native Podman container runtime for Linux
//!
//! Implements the ContainerRuntime trait using direct Podman execution
//! without a VM layer. Requires rootless Podman to be properly configured.

use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::podman::ContainerConfig;
use crate::orchestration::runtime::{ContainerRuntime, VolumeInfo};
use async_trait::async_trait;
use std::collections::HashMap;
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

        let output = self
            .exec(&["logs", "--tail", &tail_arg, container_id])
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn logs_follow(&self, container_id: &str) -> MinotaurResult<()> {
        self.exec_interactive(&["logs", "-f", container_id]).await?;
        Ok(())
    }

    fn runtime_name(&self) -> &'static str {
        "Native Podman"
    }

    async fn volume_create(
        &self,
        name: &str,
        labels: &HashMap<String, String>,
    ) -> MinotaurResult<()> {
        debug!("Creating volume: {}", name);

        let mut args = vec!["volume", "create"];

        // Build label arguments
        let label_strings: Vec<String> =
            labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        for label in &label_strings {
            args.push("--label");
            args.push(label);
        }

        args.push(name);

        let output = self.exec(&args).await?;

        if output.status.success() {
            info!("Volume created: {}", name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::command_exec("podman volume create", stderr))
        }
    }

    async fn volume_exists(&self, name: &str) -> MinotaurResult<bool> {
        let output = self.exec(&["volume", "exists", name]).await?;
        Ok(output.status.success())
    }

    async fn volume_remove(&self, name: &str) -> MinotaurResult<()> {
        debug!("Removing volume: {}", name);

        let output = self.exec(&["volume", "rm", "-f", name]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "no such volume" errors
            if stderr.contains("no such volume") {
                Ok(())
            } else {
                Err(MinotaurError::command_exec("podman volume rm", stderr))
            }
        }
    }

    async fn volume_list(&self, prefix: &str) -> MinotaurResult<Vec<VolumeInfo>> {
        // Use JSON format for reliable parsing
        let output = self.exec(&["volume", "ls", "--format", "json"]).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MinotaurError::command_exec("podman volume ls", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Parse JSON array of volumes
        let volumes: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| MinotaurError::Internal(e.to_string()))?;

        let mut result = Vec::new();
        for vol in volumes {
            let name = vol["Name"].as_str().unwrap_or_default();

            // Filter by prefix
            if !name.starts_with(prefix) {
                continue;
            }

            // Parse labels
            let labels: HashMap<String, String> = vol["Labels"]
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            result.push(VolumeInfo {
                name: name.to_string(),
                labels,
                mountpoint: vol["Mountpoint"].as_str().map(String::from),
                created_at: vol["CreatedAt"].as_str().map(String::from),
                size_bytes: None,
            });
        }

        Ok(result)
    }

    async fn volume_inspect(&self, name: &str) -> MinotaurResult<Option<VolumeInfo>> {
        let output = self
            .exec(&["volume", "inspect", name, "--format", "json"])
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no such volume") {
                return Ok(None);
            }
            return Err(MinotaurError::command_exec("podman volume inspect", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON array (inspect returns array even for single volume)
        let volumes: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| MinotaurError::Internal(e.to_string()))?;

        let vol = match volumes.first() {
            Some(v) => v,
            None => return Ok(None),
        };

        // Parse labels
        let labels: HashMap<String, String> = vol["Labels"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(VolumeInfo {
            name: name.to_string(),
            labels,
            mountpoint: vol["Mountpoint"].as_str().map(String::from),
            created_at: vol["CreatedAt"].as_str().map(String::from),
            size_bytes: None,
        }))
    }

    async fn volume_update_labels(
        &self,
        name: &str,
        labels: &HashMap<String, String>,
    ) -> MinotaurResult<()> {
        // Podman doesn't support updating labels directly
        // We need to note the limitations here - this is only safe for
        // metadata updates, not for volumes with data
        debug!("Updating volume labels: {} (recreating)", name);

        // First check if volume exists and get current info
        let existing = self.volume_inspect(name).await?;
        if existing.is_none() {
            return Err(MinotaurError::Internal(format!(
                "Volume not found: {}",
                name
            )));
        }

        // Remove old volume
        self.volume_remove(name).await?;

        // Create with new labels
        self.volume_create(name, labels).await
    }

    async fn volume_disk_usage(&self, prefix: &str) -> MinotaurResult<HashMap<String, u64>> {
        // Use podman system df -v to get volume sizes
        let output = self
            .exec(&["system", "df", "-v", "--format", "json"])
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MinotaurError::command_exec("podman system df", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Ok(HashMap::new());
        }

        // Parse JSON - structure is { "Volumes": [...] }
        let df: serde_json::Value =
            serde_json::from_str(&stdout).map_err(|e| MinotaurError::Internal(e.to_string()))?;

        let mut sizes = HashMap::new();

        if let Some(volumes) = df.get("Volumes").and_then(|v| v.as_array()) {
            for vol in volumes {
                let name = vol["VolumeName"].as_str().unwrap_or_default();

                // Filter by prefix
                if !name.starts_with(prefix) {
                    continue;
                }

                // Size is in bytes
                if let Some(size) = vol["Size"].as_u64() {
                    sizes.insert(name.to_string(), size);
                }
            }
        }

        Ok(sizes)
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
