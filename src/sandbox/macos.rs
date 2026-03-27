//! macOS native sandbox using dedicated system user + pf packet filter
//!
//! Uses `_mino_agent` system user with ACL-based filesystem access and
//! pf (packet filter) for network isolation. Requires mino-sandbox-helper
//! installed via `mino setup --native`.

use crate::error::{MinoError, MinoResult};
use crate::sandbox::helper_protocol::{AclEntry, HelperRequest, HelperResponse, ResourceLimitsDto};
use crate::sandbox::native::SandboxSpawnConfig;
use crate::sandbox::process::SandboxProcess;
use crate::sandbox::resource_limits::ResourceLimits;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::debug;

const HELPER_BINARY: &str = "mino-sandbox-helper";

/// Validate macOS prerequisites for native sandbox
pub async fn validate_macos_setup() -> MinoResult<()> {
    check_helper_installed().await?;
    check_helper_version().await?;
    check_sandbox_user_exists(&crate::sandbox::config::SandboxConfig::default().sandbox_user)
        .await?;
    Ok(())
}

async fn check_helper_installed() -> MinoResult<()> {
    let result = Command::new("which")
        .arg(HELPER_BINARY)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match result {
        Ok(status) if status.success() => Ok(()),
        _ => Err(MinoError::SandboxNotSetup),
    }
}

async fn check_helper_version() -> MinoResult<()> {
    let output = Command::new(HELPER_BINARY)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let helper_version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mino_version = env!("CARGO_PKG_VERSION");
            if helper_version != mino_version {
                return Err(MinoError::SandboxHelper(format!(
                    "Version mismatch: helper v{}, mino v{}. Run: mino setup --native --upgrade",
                    helper_version, mino_version
                )));
            }
            Ok(())
        }
        _ => Err(MinoError::SandboxNotSetup),
    }
}

async fn check_sandbox_user_exists(username: &str) -> MinoResult<()> {
    let output = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", username)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match output {
        Ok(status) if status.success() => Ok(()),
        _ => Err(MinoError::SandboxNotSetup),
    }
}

/// Build ACL entries from a SandboxSpawnConfig
pub fn build_acl_entries(config: &SandboxSpawnConfig) -> Vec<AclEntry> {
    let mut acl_paths = Vec::new();

    // Project dir: read-write
    acl_paths.push(AclEntry {
        path: config.project_dir.clone(),
        writable: true,
    });

    // Passthrough paths: read-only
    for path_str in &config.sandbox_config.passthrough_paths {
        acl_paths.push(AclEntry {
            path: PathBuf::from(path_str),
            writable: false,
        });
    }

    // Writable paths
    for path_str in &config.sandbox_config.writable_paths {
        acl_paths.push(AclEntry {
            path: PathBuf::from(path_str),
            writable: true,
        });
    }

    acl_paths
}

/// Spawn a macOS sandbox via the privileged helper
pub async fn spawn_macos_sandbox(config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
    let resource_limits = ResourceLimits::from_config(&config.sandbox_config);
    let acl_paths = build_acl_entries(&config);

    // Prepare home directory path
    let home_dir = std::env::temp_dir().join(format!("mino-home-{}", config.session_id));

    let request = HelperRequest::Spawn {
        session_id: config.session_id.clone(),
        project_dir: config.project_dir.clone(),
        env: config.env.clone(),
        command: config.command.clone(),
        resource_limits: ResourceLimitsDto::from(&resource_limits),
        acl_paths,
        dotfile_dir: config.dotfile_dir.clone(),
        home_dir,
    };

    // Write request to temp file
    let request_file = std::env::temp_dir().join(format!("mino-helper-{}.json", config.session_id));
    let request_json = serde_json::to_string(&request)?;
    tokio::fs::write(&request_file, &request_json)
        .await
        .map_err(|e| MinoError::io("writing helper request", e))?;

    // Set file permissions to 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&request_file, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|e| MinoError::io("setting request file permissions", e))?;
    }

    // Call helper via sudo
    debug!(
        "Calling helper with request file: {}",
        request_file.display()
    );
    let mut cmd = Command::new("sudo");
    cmd.arg(HELPER_BINARY);
    cmd.arg("spawn");
    cmd.arg("--request-file");
    cmd.arg(&request_file);

    if config.interactive {
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
    } else {
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    let child = cmd
        .spawn()
        .map_err(|e| MinoError::SandboxHelper(format!("Failed to spawn helper: {}", e)))?;

    // Clean up request file (best effort)
    let _ = tokio::fs::remove_file(&request_file).await;

    Ok(SandboxProcess::new(child, config.session_id))
}

/// Clean up a macOS sandbox session (ACLs, pf rules)
pub async fn cleanup_macos_sandbox(
    session_id: &str,
    project_dir: &std::path::Path,
) -> MinoResult<()> {
    let request = HelperRequest::Cleanup {
        session_id: session_id.to_string(),
        project_dir: project_dir.to_path_buf(),
    };

    let request_file = std::env::temp_dir().join(format!("mino-cleanup-{}.json", session_id));
    let request_json = serde_json::to_string(&request)?;
    tokio::fs::write(&request_file, &request_json)
        .await
        .map_err(|e| MinoError::io("writing cleanup request", e))?;

    let output = Command::new("sudo")
        .arg(HELPER_BINARY)
        .arg("cleanup")
        .arg("--request-file")
        .arg(&request_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| MinoError::SandboxHelper(format!("Failed to run cleanup: {}", e)))?;

    let _ = tokio::fs::remove_file(&request_file).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(MinoError::SandboxHelper(format!(
            "Cleanup failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Parse JSON response from helper stdout
#[allow(dead_code)]
pub fn parse_helper_response(stdout: &[u8]) -> MinoResult<HelperResponse> {
    let text = String::from_utf8_lossy(stdout);
    serde_json::from_str(text.trim()).map_err(|e| {
        MinoError::SandboxHelper(format!(
            "Failed to parse helper response: {} (raw: {})",
            e, text
        ))
    })
}

/// Generate pf anchor rules for the sandbox
///
/// Returns pf rule text that should be loaded into the "mino" anchor.
/// Rules block all outbound traffic from the sandbox user except:
/// - DNS resolution (port 53)
/// - Loopback proxy connection (if proxy_port is specified)
pub fn generate_pf_rules(sandbox_user: &str, _session_id: &str, proxy_port: Option<u16>) -> String {
    let mut rules = String::new();

    // Block all outbound TCP/UDP from the sandbox user
    rules.push_str(&format!(
        "block out quick proto {{ tcp udp }} user {}\n",
        sandbox_user
    ));

    // Allow DNS (system resolver)
    rules.push_str(&format!(
        "pass out quick proto udp to any port 53 user {}\n",
        sandbox_user
    ));
    rules.push_str(&format!(
        "pass out quick proto tcp to any port 53 user {}\n",
        sandbox_user
    ));

    // If proxy port is set, allow connection to the proxy on localhost only
    if let Some(port) = proxy_port {
        rules.push_str(&format!(
            "pass out quick proto tcp to 127.0.0.1 port {} user {}\n",
            port, sandbox_user
        ));
    }

    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::NetworkMode;
    use crate::sandbox::config::SandboxConfig;
    use std::collections::HashMap;

    #[test]
    fn pf_rules_without_proxy_blocks_all_allows_dns() {
        let rules = generate_pf_rules("_mino_agent", "sess-1", None);

        // Should block all TCP/UDP
        assert!(rules.contains("block out quick proto { tcp udp } user _mino_agent"));
        // Should allow DNS on UDP and TCP
        assert!(rules.contains("pass out quick proto udp to any port 53 user _mino_agent"));
        assert!(rules.contains("pass out quick proto tcp to any port 53 user _mino_agent"));
        // Should NOT have any localhost port rule
        assert!(!rules.contains("127.0.0.1 port"));
    }

    #[test]
    fn pf_rules_with_proxy_allows_localhost_port() {
        let rules = generate_pf_rules("_mino_agent", "sess-1", Some(8080));

        // Should have the block rule
        assert!(rules.contains("block out quick proto { tcp udp } user _mino_agent"));
        // Should have DNS rules
        assert!(rules.contains("port 53"));
        // Should allow proxy port on localhost
        assert!(rules.contains("pass out quick proto tcp to 127.0.0.1 port 8080 user _mino_agent"));
    }

    #[test]
    fn pf_rules_uses_correct_user_name() {
        let rules = generate_pf_rules("custom_sandbox_user", "sess-1", None);

        assert!(rules.contains("user custom_sandbox_user"));
        assert!(!rules.contains("_mino_agent"));
    }

    #[test]
    fn build_acl_entries_from_config() {
        let config = SandboxSpawnConfig {
            session_id: "test-sess".to_string(),
            project_dir: PathBuf::from("/home/user/project"),
            command: vec!["bash".to_string()],
            env: HashMap::new(),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig {
                passthrough_paths: vec!["/usr/share/data".to_string()],
                writable_paths: vec!["/tmp/scratch".to_string()],
                ..Default::default()
            },
            dotfile_dir: None,
            interactive: false,
        };

        let acl_entries = build_acl_entries(&config);

        assert_eq!(acl_entries.len(), 3);

        // Project dir is read-write
        assert_eq!(acl_entries[0].path, PathBuf::from("/home/user/project"));
        assert!(acl_entries[0].writable);

        // Passthrough path is read-only
        assert_eq!(acl_entries[1].path, PathBuf::from("/usr/share/data"));
        assert!(!acl_entries[1].writable);

        // Writable path is read-write
        assert_eq!(acl_entries[2].path, PathBuf::from("/tmp/scratch"));
        assert!(acl_entries[2].writable);
    }

    #[test]
    fn build_acl_entries_project_dir_only() {
        let config = SandboxSpawnConfig {
            session_id: "test".to_string(),
            project_dir: PathBuf::from("/tmp/project"),
            command: vec!["true".to_string()],
            env: HashMap::new(),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig::default(),
            dotfile_dir: None,
            interactive: false,
        };

        let acl_entries = build_acl_entries(&config);

        // Only project dir when no passthrough/writable paths configured
        assert_eq!(acl_entries.len(), 1);
        assert_eq!(acl_entries[0].path, PathBuf::from("/tmp/project"));
        assert!(acl_entries[0].writable);
    }

    #[test]
    fn spawn_request_serialization_includes_all_fields() {
        let config = SandboxSpawnConfig {
            session_id: "serialize-test".to_string(),
            project_dir: PathBuf::from("/tmp/proj"),
            command: vec!["bash".to_string()],
            env: HashMap::from([("KEY".to_string(), "val".to_string())]),
            network_mode: NetworkMode::Bridge,
            sandbox_config: SandboxConfig {
                passthrough_paths: vec!["/opt/data".to_string()],
                ..Default::default()
            },
            dotfile_dir: Some(PathBuf::from("/tmp/dots")),
            interactive: true,
        };

        let resource_limits = ResourceLimits::from_config(&config.sandbox_config);
        let acl_paths = build_acl_entries(&config);

        let request = HelperRequest::Spawn {
            session_id: config.session_id.clone(),
            project_dir: config.project_dir.clone(),
            env: config.env.clone(),
            command: config.command.clone(),
            resource_limits: ResourceLimitsDto::from(&resource_limits),
            acl_paths,
            dotfile_dir: config.dotfile_dir.clone(),
            home_dir: PathBuf::from("/tmp/mino-home-serialize-test"),
        };

        let json = serde_json::to_string(&request).unwrap();

        // All fields present in JSON
        assert!(json.contains("serialize-test"));
        assert!(json.contains("/tmp/proj"));
        assert!(json.contains("KEY"));
        assert!(json.contains("bash"));
        assert!(json.contains("max_memory_bytes"));
        assert!(json.contains("/opt/data"));
        assert!(json.contains("/tmp/dots"));
        assert!(json.contains("/tmp/mino-home-serialize-test"));
    }

    #[test]
    fn parse_helper_response_spawned() {
        let json = r#"{"status":"Spawned","pid":42}"#;
        let resp = parse_helper_response(json.as_bytes()).unwrap();
        match resp {
            HelperResponse::Spawned { pid } => assert_eq!(pid, 42),
            _ => panic!("expected Spawned"),
        }
    }

    #[test]
    fn parse_helper_response_error() {
        let json = r#"{"status":"Error","message":"fork failed"}"#;
        let resp = parse_helper_response(json.as_bytes()).unwrap();
        match resp {
            HelperResponse::Error { message } => assert_eq!(message, "fork failed"),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn parse_helper_response_invalid_json() {
        let result = parse_helper_response(b"not json");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse helper response"));
    }
}
