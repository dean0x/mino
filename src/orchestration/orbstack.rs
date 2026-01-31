//! OrbStack VM management

use crate::config::schema::VmConfig;
use crate::error::{MinotaurError, MinotaurResult};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info};

/// OrbStack manager
#[derive(Clone)]
pub struct OrbStack {
    config: VmConfig,
}

impl OrbStack {
    /// Create a new OrbStack manager
    pub fn new(config: VmConfig) -> Self {
        Self { config }
    }

    /// Check if OrbStack is installed
    pub async fn is_installed() -> bool {
        Command::new("orb")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Check if OrbStack is running
    pub async fn is_running() -> MinotaurResult<bool> {
        let output = Command::new("orb")
            .args(["status"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("orb status", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.contains("running") || output.status.success())
    }

    /// Get OrbStack version
    pub async fn version() -> MinotaurResult<String> {
        let output = Command::new("orb")
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("orb --version", e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(MinotaurError::OrbStackNotFound)
        }
    }

    /// Start OrbStack
    pub async fn start() -> MinotaurResult<()> {
        info!("Starting OrbStack...");

        let status = Command::new("orb")
            .arg("start")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .map_err(|e| MinotaurError::command_failed("orb start", e))?;

        if status.success() {
            Ok(())
        } else {
            Err(MinotaurError::VmStart("Failed to start OrbStack".to_string()))
        }
    }

    /// Check if the VM exists
    pub async fn vm_exists(&self) -> MinotaurResult<bool> {
        let output = Command::new("orb")
            .args(["list", "-f", "{{.Name}}"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("orb list", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().any(|line| line.trim() == self.config.name))
    }

    /// Create the VM
    pub async fn create_vm(&self) -> MinotaurResult<()> {
        info!("Creating OrbStack VM: {}", self.config.name);

        let mut cmd = Command::new("orb");
        cmd.args(["create", &self.config.distro, &self.config.name]);

        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("orb create", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::VmStart(format!("Failed to create VM: {}", stderr)))
        }
    }

    /// Ensure VM is running
    pub async fn ensure_vm_running(&self) -> MinotaurResult<()> {
        // First ensure OrbStack itself is running
        if !Self::is_running().await? {
            Self::start().await?;
        }

        // Check if VM exists
        if !self.vm_exists().await? {
            self.create_vm().await?;
        }

        // Start VM if needed
        let status = self.vm_status().await?;
        if status != "running" {
            self.start_vm().await?;
        }

        Ok(())
    }

    /// Get VM status
    pub async fn vm_status(&self) -> MinotaurResult<String> {
        let output = Command::new("orb")
            .args(["list", "-f", "{{.Name}}\t{{.State}}"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed("orb list", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 && parts[0].trim() == self.config.name {
                return Ok(parts[1].trim().to_string());
            }
        }

        Ok("unknown".to_string())
    }

    /// Start the VM
    pub async fn start_vm(&self) -> MinotaurResult<()> {
        info!("Starting VM: {}", self.config.name);

        let status = Command::new("orb")
            .args(["start", &self.config.name])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .map_err(|e| MinotaurError::command_failed("orb start", e))?;

        if status.success() {
            Ok(())
        } else {
            Err(MinotaurError::VmStart(format!("Failed to start VM: {}", self.config.name)))
        }
    }

    /// Execute a command in the VM
    pub async fn exec(&self, command: &[&str]) -> MinotaurResult<std::process::Output> {
        debug!("Executing in VM {}: {:?}", self.config.name, command);

        let mut cmd = Command::new("orb");
        cmd.arg("-m").arg(&self.config.name);
        cmd.args(command);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| MinotaurError::command_failed(format!("orb -m {} {:?}", self.config.name, command), e))?;

        Ok(output)
    }

    /// Execute a command in the VM and return stdout
    pub async fn exec_output(&self, command: &[&str]) -> MinotaurResult<String> {
        let output = self.exec(command).await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MinotaurError::VmCommand(format!(
                "Command failed: {:?}, stderr: {}",
                command, stderr
            )))
        }
    }

    /// Execute a command in the VM interactively
    pub async fn exec_interactive(&self, command: &[&str]) -> MinotaurResult<i32> {
        debug!("Executing interactively in VM {}: {:?}", self.config.name, command);

        let mut cmd = Command::new("orb");
        cmd.arg("-m").arg(&self.config.name);
        cmd.args(command);
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let status = cmd
            .status()
            .await
            .map_err(|e| MinotaurError::command_failed(format!("orb -m {} {:?}", self.config.name, command), e))?;

        Ok(status.code().unwrap_or(-1))
    }

    /// Get VM name
    pub fn vm_name(&self) -> &str {
        &self.config.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbstack_new() {
        let config = VmConfig::default();
        let orb = OrbStack::new(config);
        assert_eq!(orb.vm_name(), "minotaur");
    }
}
