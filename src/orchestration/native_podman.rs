//! Native Podman container runtime for Linux
//!
//! Implements the ContainerRuntime trait using direct Podman execution
//! without a VM layer. Requires rootless Podman to be properly configured.

use crate::error::{MinoError, MinoResult};
use crate::orchestration::podman::{redact_args, ContainerConfig};
use crate::orchestration::runtime::{ContainerRuntime, VolumeInfo};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, warn};

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
    async fn rootless_configured() -> MinoResult<bool> {
        // Check if user namespaces are available
        let output = Command::new("podman")
            .args(["info", "--format", "{{.Host.Security.Rootless}}"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinoError::command_failed("podman info", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim() == "true")
    }

    /// Execute a Podman command and return the output
    async fn exec(&self, args: &[&str]) -> MinoResult<std::process::Output> {
        debug!("Executing: podman {:?}", redact_args(args));

        Command::new("podman")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinoError::command_failed(format!("podman {:?}", redact_args(args)), e))
    }

    /// Execute a Podman command interactively
    async fn exec_interactive(&self, args: &[&str]) -> MinoResult<i32> {
        debug!("Executing interactively: podman {:?}", redact_args(args));

        let status = Command::new("podman")
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .map_err(|e| MinoError::command_failed(format!("podman {:?}", redact_args(args)), e))?;

        Ok(status.code().unwrap_or(-1))
    }

    /// Pull an image
    async fn pull(&self, image: &str) -> MinoResult<()> {
        debug!("Pulling image: {}", image);

        let output = self.exec(&["pull", image]).await?;

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

impl Default for NativePodmanRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContainerRuntime for NativePodmanRuntime {
    async fn is_available(&self) -> MinoResult<bool> {
        if !Self::podman_installed().await {
            return Ok(false);
        }
        Self::rootless_configured().await
    }

    async fn ensure_ready(&self) -> MinoResult<()> {
        if !Self::podman_installed().await {
            return Err(MinoError::PodmanNotFound);
        }

        if !Self::rootless_configured().await? {
            return Err(MinoError::PodmanRootlessSetup {
                reason: "Rootless Podman not configured. Run: podman system migrate".to_string(),
            });
        }

        Ok(())
    }

    async fn run(&self, config: &ContainerConfig, command: &[String]) -> MinoResult<String> {
        // Ensure image is available
        if !self.image_exists(&config.image).await? {
            self.pull(&config.image).await?;
        }

        let mut args = vec!["run".to_string(), "-d".to_string()];

        if config.interactive {
            args.push("-i".to_string());
        }
        if config.tty {
            args.push("-t".to_string());
        }

        config.push_args(&mut args, command);

        debug!(
            "Running container (detached): podman {:?}",
            redact_args(&args)
        );

        let args_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.exec(&args_refs).await?;

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

        let mut args = vec!["create".to_string()];

        if config.interactive {
            args.push("-i".to_string());
        }
        if config.tty {
            args.push("-t".to_string());
        }

        config.push_args(&mut args, command);

        debug!("Creating container: podman {:?}", redact_args(&args));

        let args_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.exec(&args_refs).await?;

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
        self.exec_interactive(&["start", "--attach", container_id])
            .await
    }

    async fn stop(&self, container_id: &str) -> MinoResult<()> {
        debug!("Stopping container: {}", container_id);

        let output = self.exec(&["stop", container_id]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::command_exec("podman stop", stderr))
        }
    }

    async fn kill(&self, container_id: &str) -> MinoResult<()> {
        debug!("Killing container: {}", container_id);

        let output = self.exec(&["kill", container_id]).await?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::command_exec("podman kill", stderr))
        }
    }

    async fn remove(&self, container_id: &str) -> MinoResult<()> {
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
                Err(MinoError::command_exec("podman rm", stderr))
            }
        }
    }

    async fn container_prune(&self) -> MinoResult<()> {
        let output = self.exec(&["container", "prune", "-f"]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MinoError::command_exec("podman container prune", stderr));
        }
        Ok(())
    }

    async fn logs(&self, container_id: &str, lines: u32) -> MinoResult<String> {
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

    async fn logs_follow(&self, container_id: &str) -> MinoResult<()> {
        self.exec_interactive(&["logs", "-f", container_id]).await?;
        Ok(())
    }

    async fn image_exists(&self, image: &str) -> MinoResult<bool> {
        let output = self.exec(&["image", "exists", image]).await?;
        Ok(output.status.success())
    }

    async fn build_image(&self, context_dir: &Path, tag: &str) -> MinoResult<()> {
        let context_str = context_dir.display().to_string();
        let output = self.exec(&["build", "-t", tag, &context_str]).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = super::build_error_output(&stdout, &stderr);
            return Err(MinoError::ImageBuild {
                tag: tag.to_string(),
                reason: combined,
            });
        }

        Ok(())
    }

    async fn build_image_with_progress(
        &self,
        context_dir: &Path,
        tag: &str,
        on_output: &(dyn Fn(String) + Send + Sync),
    ) -> MinoResult<()> {
        let context_str = context_dir.display().to_string();

        let mut child = Command::new("podman")
            .args(["build", "-t", tag, &context_str])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MinoError::command_failed("podman build", e))?;

        let all_output = super::stream_child_output(&mut child, on_output).await;

        let status = child
            .wait()
            .await
            .map_err(|e| MinoError::command_failed("podman build", e))?;

        if !status.success() {
            let combined = all_output.join("\n");
            let tail = super::build_error_output(&combined, "");
            return Err(MinoError::ImageBuild {
                tag: tag.to_string(),
                reason: tail,
            });
        }

        Ok(())
    }

    async fn image_remove(&self, image: &str) -> MinoResult<()> {
        let output = self.exec(&["rmi", image]).await?;

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
            .exec(&[
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
        "Native Podman"
    }

    async fn volume_create(&self, name: &str, labels: &HashMap<String, String>) -> MinoResult<()> {
        debug!("Creating volume: {}", name);

        let mut args = vec!["volume", "create", "--ignore"];

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
            debug!("Volume created: {}", name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinoError::command_exec("podman volume create", stderr))
        }
    }

    async fn volume_remove(&self, name: &str) -> MinoResult<()> {
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
                Err(MinoError::command_exec("podman volume rm", stderr))
            }
        }
    }

    async fn volume_list(&self, prefix: &str) -> MinoResult<Vec<VolumeInfo>> {
        // Use JSON format for reliable parsing
        let output = self.exec(&["volume", "ls", "--format", "json"]).await?;

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
            .exec(&["volume", "inspect", name, "--format", "json"])
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

    async fn volume_disk_usage(&self, prefix: &str) -> MinoResult<HashMap<String, u64>> {
        // Get volume sizes by inspecting each volume individually.
        // Note: `podman system df -v --format json` is not supported (flags conflict).
        let volumes = self.volume_list(prefix).await?;

        let futures = volumes.into_iter().map(|vol| async move {
            let output = self
                .exec(&[
                    "volume",
                    "inspect",
                    &vol.name,
                    "--format",
                    "{{.Mountpoint}}",
                ])
                .await?;

            if !output.status.success() {
                return Ok(None);
            }

            let mountpoint = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if mountpoint.is_empty() {
                return Ok(None);
            }

            let du_output = tokio::process::Command::new("du")
                .args(["-sb", &mountpoint])
                .output()
                .await
                .map_err(|e| MinoError::io("du", e))?;

            if du_output.status.success() {
                if let Some(size) = super::parse_du_bytes(&du_output.stdout) {
                    return Ok(Some((vol.name.clone(), size)));
                }
            }

            Ok(None)
        });

        let results: Vec<MinoResult<Option<(String, u64)>>> =
            futures::future::join_all(futures).await;

        super::collect_disk_usage(results)
    }

    async fn get_container_exit_code(&self, container_id: &str) -> MinoResult<Option<i32>> {
        debug!("Waiting for container exit: {}", container_id);

        let output = self.exec(&["wait", container_id]).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no such container") {
                return Ok(None);
            }
            return Err(MinoError::command_exec("podman wait", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        match stdout.trim().parse::<i32>() {
            Ok(code) => Ok(Some(code)),
            Err(_) => {
                warn!(
                    "Could not parse exit code from podman wait: {:?}",
                    stdout.trim()
                );
                Ok(None)
            }
        }
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
