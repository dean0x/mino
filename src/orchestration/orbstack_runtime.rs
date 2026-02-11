//! OrbStack container runtime for macOS
//!
//! Implements the ContainerRuntime trait using OrbStack VM + Podman.

use crate::config::schema::VmConfig;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::orbstack::OrbStack;
use crate::orchestration::podman::ContainerConfig;
use crate::orchestration::runtime::{ContainerRuntime, VolumeInfo};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

/// Container runtime using OrbStack VM + Podman (for macOS)
pub struct OrbStackRuntime {
    orbstack: OrbStack,
}

impl OrbStackRuntime {
    /// Create a new OrbStack runtime
    pub fn new(vm_config: VmConfig) -> Self {
        Self {
            orbstack: OrbStack::new(vm_config),
        }
    }

    /// Check if Podman is available in the VM
    async fn podman_available(&self) -> MinoResult<bool> {
        let output = self.orbstack.exec(&["which", "podman"]).await?;
        Ok(output.status.success())
    }

    /// Install Podman in the VM if not present
    async fn ensure_podman(&self) -> MinoResult<()> {
        if self.podman_available().await? {
            return Ok(());
        }

        debug!("Installing Podman in VM...");

        // Try to install based on distro
        let install_result = self
            .orbstack
            .exec(&["sudo", "dnf", "install", "-y", "podman"])
            .await?;

        if !install_result.status.success() {
            // Try apt as fallback
            let apt_result = self
                .orbstack
                .exec(&["sudo", "apt-get", "install", "-y", "podman"])
                .await?;

            if !apt_result.status.success() {
                return Err(MinoError::PodmanNotFound);
            }
        }

        Ok(())
    }

    /// Pull an image
    async fn pull(&self, image: &str) -> MinoResult<()> {
        debug!("Pulling image: {}", image);

        let output = self.orbstack.exec(&["podman", "pull", image]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::ImagePull {
                image: image.to_string(),
                reason: stderr.to_string(),
            })
        }
    }
}

#[async_trait]
impl ContainerRuntime for OrbStackRuntime {
    async fn is_available(&self) -> MinoResult<bool> {
        if !OrbStack::is_installed().await {
            return Ok(false);
        }
        if !OrbStack::is_running().await? {
            return Ok(false);
        }
        self.podman_available().await
    }

    async fn ensure_ready(&self) -> MinoResult<()> {
        self.orbstack.ensure_vm_running().await?;
        self.ensure_podman().await
    }

    async fn run(&self, config: &ContainerConfig, command: &[String]) -> MinoResult<String> {
        // Ensure image is available
        if !self.image_exists(&config.image).await? {
            self.pull(&config.image).await?;
        }

        let mut args = vec!["podman", "run", "-d"];

        if config.interactive {
            args.push("-i");
        }
        if config.tty {
            args.push("-t");
        }

        args.push("-w");
        args.push(&config.workdir);
        args.push("--network");
        args.push(&config.network);

        let volume_args: Vec<String> = config
            .volumes
            .iter()
            .flat_map(|v| vec!["-v".to_string(), v.clone()])
            .collect();
        let env_args: Vec<String> = config
            .env
            .iter()
            .flat_map(|(k, v)| vec!["-e".to_string(), format!("{}={}", k, v)])
            .collect();

        let mut cmd_args: Vec<&str> = args.clone();
        for v in &volume_args {
            cmd_args.push(v);
        }
        for e in &env_args {
            cmd_args.push(e);
        }
        cmd_args.push(&config.image);
        for c in command {
            cmd_args.push(c);
        }

        debug!("Running container (detached): {:?}", cmd_args);

        let output = self.orbstack.exec(&cmd_args).await?;

        if output.status.success() {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            debug!(
                "Container started: {}",
                &container_id[..12.min(container_id.len())]
            );
            Ok(container_id)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::ContainerStart(stderr.to_string()))
        }
    }

    async fn create(&self, config: &ContainerConfig, command: &[String]) -> MinoResult<String> {
        // Ensure image is available
        if !self.image_exists(&config.image).await? {
            self.pull(&config.image).await?;
        }

        let mut args = vec!["podman", "create"];

        if config.interactive {
            args.push("-i");
        }
        if config.tty {
            args.push("-t");
        }

        args.push("-w");
        args.push(&config.workdir);
        args.push("--network");
        args.push(&config.network);

        let volume_args: Vec<String> = config
            .volumes
            .iter()
            .flat_map(|v| vec!["-v".to_string(), v.clone()])
            .collect();
        let env_args: Vec<String> = config
            .env
            .iter()
            .flat_map(|(k, v)| vec!["-e".to_string(), format!("{}={}", k, v)])
            .collect();

        let mut cmd_args: Vec<&str> = args.clone();
        for v in &volume_args {
            cmd_args.push(v);
        }
        for e in &env_args {
            cmd_args.push(e);
        }
        cmd_args.push(&config.image);
        for c in command {
            cmd_args.push(c);
        }

        debug!("Creating container: {:?}", cmd_args);

        let output = self.orbstack.exec(&cmd_args).await?;

        if output.status.success() {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            debug!(
                "Container created: {}",
                &container_id[..12.min(container_id.len())]
            );
            Ok(container_id)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::ContainerStart(stderr.to_string()))
        }
    }

    async fn start_attached(&self, container_id: &str) -> MinoResult<i32> {
        debug!("Starting container attached: {}", container_id);

        let exit_code = self
            .orbstack
            .exec_interactive(&["podman", "start", "--attach", container_id])
            .await?;

        Ok(exit_code)
    }

    async fn attach(&self, container_id: &str) -> MinoResult<i32> {
        debug!("Attaching to container: {}", container_id);

        let exit_code = self
            .orbstack
            .exec_interactive(&["podman", "attach", container_id])
            .await?;

        Ok(exit_code)
    }

    async fn stop(&self, container_id: &str) -> MinoResult<()> {
        debug!("Stopping container: {}", container_id);

        let output = self
            .orbstack
            .exec(&["podman", "stop", container_id])
            .await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::command_exec("podman stop", stderr))
        }
    }

    async fn kill(&self, container_id: &str) -> MinoResult<()> {
        debug!("Killing container: {}", container_id);

        let output = self
            .orbstack
            .exec(&["podman", "kill", container_id])
            .await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::command_exec("podman kill", stderr))
        }
    }

    async fn remove(&self, container_id: &str) -> MinoResult<()> {
        debug!("Removing container: {}", container_id);

        let output = self
            .orbstack
            .exec(&["podman", "rm", "-f", container_id])
            .await?;

        if output.status.success() {
            Ok(())
        } else {
            // Ignore error if container doesn't exist
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no such container") {
                Ok(())
            } else {
                Err(MinoError::command_exec("podman rm", stderr))
            }
        }
    }

    async fn logs(&self, container_id: &str, lines: u32) -> MinoResult<String> {
        let tail_arg = if lines == 0 {
            "all".to_string()
        } else {
            lines.to_string()
        };

        let output = self
            .orbstack
            .exec(&["podman", "logs", "--tail", &tail_arg, container_id])
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn logs_follow(&self, container_id: &str) -> MinoResult<()> {
        self.orbstack
            .exec_interactive(&["podman", "logs", "-f", container_id])
            .await?;
        Ok(())
    }

    async fn image_exists(&self, image: &str) -> MinoResult<bool> {
        let output = self
            .orbstack
            .exec(&["podman", "image", "exists", image])
            .await?;
        Ok(output.status.success())
    }

    async fn build_image(&self, context_dir: &Path, tag: &str) -> MinoResult<()> {
        let context_str = context_dir.display().to_string();
        let exit_code = self
            .orbstack
            .exec_interactive(&["podman", "build", "-t", tag, &context_str])
            .await?;

        if exit_code != 0 {
            return Err(MinoError::ImageBuild {
                tag: tag.to_string(),
                reason: format!("build exited with code {}", exit_code),
            });
        }

        Ok(())
    }

    async fn image_remove(&self, image: &str) -> MinoResult<()> {
        let output = self.orbstack.exec(&["podman", "rmi", image]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("image not known") {
                Ok(())
            } else {
                Err(MinoError::command_exec("podman rmi", stderr))
            }
        }
    }

    async fn image_list_prefixed(&self, prefix: &str) -> MinoResult<Vec<String>> {
        let filter = format!("reference={}*", prefix);
        let output = self
            .orbstack
            .exec(&[
                "podman",
                "images",
                "--filter",
                &filter,
                "--format",
                "{{.Repository}}:{{.Tag}}",
            ])
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MinoError::command_exec("podman images", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let images: Vec<String> = stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(String::from)
            .collect();

        Ok(images)
    }

    fn runtime_name(&self) -> &'static str {
        "OrbStack + Podman"
    }

    async fn volume_create(
        &self,
        name: &str,
        labels: &HashMap<String, String>,
    ) -> MinoResult<()> {
        debug!("Creating volume: {}", name);

        let mut args = vec!["podman", "volume", "create", "--ignore"];

        // Build label arguments
        let label_strings: Vec<String> =
            labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        for label in &label_strings {
            args.push("--label");
            args.push(label);
        }

        args.push(name);

        let output = self.orbstack.exec(&args).await?;

        if output.status.success() {
            debug!("Volume created: {}", name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::command_exec("podman volume create", stderr))
        }
    }

    async fn volume_exists(&self, name: &str) -> MinoResult<bool> {
        let output = self
            .orbstack
            .exec(&["podman", "volume", "exists", name])
            .await?;
        Ok(output.status.success())
    }

    async fn volume_remove(&self, name: &str) -> MinoResult<()> {
        debug!("Removing volume: {}", name);

        let output = self
            .orbstack
            .exec(&["podman", "volume", "rm", "-f", name])
            .await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "no such volume" errors
            if stderr.contains("no such volume") {
                Ok(())
            } else {
                Err(MinoError::command_exec("podman volume rm", stderr))
            }
        }
    }

    async fn volume_list(&self, prefix: &str) -> MinoResult<Vec<VolumeInfo>> {
        // Use JSON format for reliable parsing
        let output = self
            .orbstack
            .exec(&["podman", "volume", "ls", "--format", "json"])
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MinoError::command_exec("podman volume ls", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Parse JSON array of volumes
        let volumes: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| MinoError::Internal(e.to_string()))?;

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

    async fn volume_inspect(&self, name: &str) -> MinoResult<Option<VolumeInfo>> {
        let output = self
            .orbstack
            .exec(&["podman", "volume", "inspect", name, "--format", "json"])
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no such volume") {
                return Ok(None);
            }
            return Err(MinoError::command_exec("podman volume inspect", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON array (inspect returns array even for single volume)
        let volumes: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| MinoError::Internal(e.to_string()))?;

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
    ) -> MinoResult<()> {
        debug!("Updating volume labels: {} (recreating)", name);

        // First check if volume exists
        let existing = self.volume_inspect(name).await?;
        if existing.is_none() {
            return Err(MinoError::Internal(format!(
                "Volume not found: {}",
                name
            )));
        }

        // Remove old volume
        self.volume_remove(name).await?;

        // Create with new labels
        self.volume_create(name, labels).await
    }

    async fn volume_disk_usage(&self, prefix: &str) -> MinoResult<HashMap<String, u64>> {
        // Get volume sizes by inspecting each volume individually.
        // Note: `podman system df -v --format json` is not supported (flags conflict).
        let volumes = self.volume_list(prefix).await?;
        let mut sizes = HashMap::new();

        for vol in &volumes {
            let output = self
                .orbstack
                .exec(&[
                    "podman",
                    "volume",
                    "inspect",
                    &vol.name,
                    "--format",
                    "{{.Mountpoint}}",
                ])
                .await?;

            if output.status.success() {
                let mountpoint = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !mountpoint.is_empty() {
                    // Get directory size via du
                    let du_output = self.orbstack.exec(&["du", "-sb", &mountpoint]).await?;
                    if du_output.status.success() {
                        let du_str = String::from_utf8_lossy(&du_output.stdout);
                        if let Some(size_str) = du_str.split_whitespace().next() {
                            if let Ok(size) = size_str.parse::<u64>() {
                                sizes.insert(vol.name.clone(), size);
                            }
                        }
                    }
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
    fn orbstack_runtime_new() {
        let config = VmConfig::default();
        let runtime = OrbStackRuntime::new(config);
        assert_eq!(runtime.runtime_name(), "OrbStack + Podman");
    }
}
