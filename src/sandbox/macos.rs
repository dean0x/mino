//! macOS native sandbox using dedicated system user + pf packet filter
//!
//! Uses `_mino_agent` system user with ACL-based filesystem access and
//! pf (packet filter) for network isolation. Requires mino-sandbox-helper
//! installed via `mino setup --native`.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::debug;

use crate::error::{MinoError, MinoResult};
use crate::sandbox::helper_protocol::{AclEntry, HelperRequest, ResourceLimitsDto};
#[cfg(test)]
use crate::sandbox::helper_protocol::HelperResponse;
use crate::sandbox::native::{SandboxPlatform, SandboxSpawnConfig};
use crate::sandbox::process::SandboxProcess;
use crate::sandbox::resource_limits::ResourceLimits;

/// macOS sandbox implementation using dedicated user + pf packet filter.
pub struct MacosSandbox;

#[async_trait]
impl SandboxPlatform for MacosSandbox {
    async fn validate_setup(&self) -> MinoResult<()> {
        validate_macos_setup().await
    }

    async fn spawn(&self, config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
        spawn_macos_sandbox(config).await
    }

    async fn exec(
        &self,
        pid: u32,
        session_name: &str,
        sandbox_user: &str,
        command: &[String],
    ) -> MinoResult<i32> {
        exec_macos(pid, session_name, sandbox_user, command).await
    }

    async fn cleanup(
        &self,
        session_id: &str,
        project_dir: &Path,
        sandbox_user: &str,
    ) -> MinoResult<()> {
        cleanup_macos_sandbox(session_id, project_dir, sandbox_user).await
    }
}

/// Execute a command inside a macOS sandbox via the helper binary.
async fn exec_macos(
    pid: u32,
    session_name: &str,
    sandbox_user: &str,
    command: &[String],
) -> MinoResult<i32> {
    let status = Command::new("sudo")
        .arg(HELPER_BINARY)
        .arg("exec")
        .arg("--session-id")
        .arg(session_name)
        .arg("--sandbox-user")
        .arg(sandbox_user)
        .arg("--pid")
        .arg(pid.to_string())
        .arg("--")
        .args(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|e| MinoError::command_failed("mino-sandbox-helper exec", e))?;

    Ok(status.code().unwrap_or(128))
}

const HELPER_BINARY: &str = "/usr/local/bin/mino-sandbox-helper";

/// Validate macOS prerequisites for native sandbox
pub(crate) async fn validate_macos_setup() -> MinoResult<()> {
    // Run independent checks in parallel (installed + user exists)
    let (installed, user) = tokio::join!(
        check_helper_installed(),
        check_sandbox_user_exists(crate::sandbox::config::DEFAULT_SANDBOX_USER),
    );
    installed?;
    // Version check depends on helper being installed, so run sequentially
    check_helper_version().await?;
    user?;
    Ok(())
}

async fn check_helper_installed() -> MinoResult<()> {
    if tokio::fs::metadata(HELPER_BINARY).await.is_ok() {
        Ok(())
    } else {
        Err(MinoError::SandboxNotSetup)
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
pub(crate) fn build_acl_entries(config: &SandboxSpawnConfig) -> Vec<AclEntry> {
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
pub(crate) async fn spawn_macos_sandbox(config: SandboxSpawnConfig) -> MinoResult<SandboxProcess> {
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
        sandbox_user: config.sandbox_config.sandbox_user.clone(),
    };

    // Write request to temp file with restricted permissions from the start
    // to avoid TOCTOU race where the file is world-readable before chmod.
    let request_file = std::env::temp_dir().join(format!("mino-helper-{}.json", config.session_id));
    let request_json = serde_json::to_string(&request)?;
    write_restricted_file(&request_file, &request_json).await?;

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

    // NOTE: We intentionally do NOT delete request_file here. The helper
    // process may not have read it yet (spawn() returns as soon as the child
    // is created, before it calls read_to_string). The file is mode 0o600 so
    // only the owning user and root can read it. It will be cleaned up on
    // the next OS tmpdir purge or when mino cleanup runs.

    Ok(SandboxProcess::new(child, config.session_id))
}

/// Clean up a macOS sandbox session (ACLs, pf rules)
pub(crate) async fn cleanup_macos_sandbox(
    session_id: &str,
    project_dir: &Path,
    sandbox_user: &str,
) -> MinoResult<()> {
    let request = HelperRequest::Cleanup {
        session_id: session_id.to_string(),
        project_dir: project_dir.to_path_buf(),
        sandbox_user: sandbox_user.to_string(),
    };

    let request_file = std::env::temp_dir().join(format!("mino-cleanup-{}.json", session_id));
    let request_json = serde_json::to_string(&request)?;
    write_restricted_file(&request_file, &request_json).await?;

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
#[cfg(test)]
fn parse_helper_response(stdout: &[u8]) -> MinoResult<HelperResponse> {
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
///
/// Returns an error if `sandbox_user` contains characters that could inject pf rules.
pub(crate) fn generate_pf_rules(
    sandbox_user: &str,
    _session_id: &str,
    proxy_port: Option<u16>,
) -> MinoResult<String> {
    crate::sandbox::config::validate_sandbox_user(sandbox_user)?;

    let mut rules = String::new();

    // Pass rules MUST come before the block rule because pf evaluates rules
    // in order and `quick` stops processing on first match. If the block rule
    // came first, DNS and proxy pass rules would be unreachable.

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

    // Block all remaining outbound TCP/UDP from the sandbox user
    rules.push_str(&format!(
        "block out quick proto {{ tcp udp }} user {}\n",
        sandbox_user
    ));

    Ok(rules)
}

/// Write content to a file with mode 0o600 from creation, avoiding TOCTOU races.
///
/// Uses `create_new(true)` (O_CREAT | O_EXCL) so the open fails if the file
/// already exists, preventing symlink attacks where an attacker pre-creates
/// a symlink at the target path.
///
/// If the file already exists (e.g., stale from a previous crash), verifies it
/// is not a symlink before removing it and retrying. This closes the TOCTOU
/// window that would exist if we blindly removed before creating.
async fn write_restricted_file(path: &std::path::Path, content: &str) -> MinoResult<()> {
    use std::fs::OpenOptions;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let path = path.to_path_buf();
    let content = content.to_string();
    let result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;

        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);

        match opts.open(&path) {
            Ok(mut f) => {
                f.write_all(content.as_bytes())?;
                Ok(())
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Stale file from a previous run — verify it is not a symlink
                // before removing to prevent symlink-following attacks.
                if let Ok(meta) = std::fs::symlink_metadata(&path) {
                    if meta.file_type().is_symlink() {
                        return Err(std::io::Error::other(format!(
                            "refusing to overwrite symlink at {}",
                            path.display()
                        )));
                    }
                }
                std::fs::remove_file(&path)?;
                let mut f = opts.open(&path)?;
                f.write_all(content.as_bytes())?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| MinoError::io("spawning restricted file writer", e.into()))?;
    result.map_err(|e| MinoError::io("writing restricted file", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::NetworkMode;
    use crate::sandbox::config::SandboxConfig;
    use std::collections::HashMap;

    #[test]
    fn pf_rules_without_proxy_blocks_all_allows_dns() {
        let rules = generate_pf_rules("_mino_agent", "sess-1", None).unwrap();

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
        let rules = generate_pf_rules("_mino_agent", "sess-1", Some(8080)).unwrap();

        // Should have the block rule
        assert!(rules.contains("block out quick proto { tcp udp } user _mino_agent"));
        // Should have DNS rules
        assert!(rules.contains("port 53"));
        // Should allow proxy port on localhost
        assert!(rules.contains("pass out quick proto tcp to 127.0.0.1 port 8080 user _mino_agent"));
    }

    #[test]
    fn pf_rules_uses_correct_user_name() {
        let rules = generate_pf_rules("custom_sandbox_user", "sess-1", None).unwrap();

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
            sandbox_user: config.sandbox_config.sandbox_user.clone(),
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

    // ---- pf_rules injection safety tests ----

    #[test]
    fn pf_rules_user_with_spaces_returns_error() {
        // A username with spaces would break pf rule syntax — must be rejected
        let err = generate_pf_rules("bad user", "sess-1", None).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn pf_rules_user_with_newline_returns_error() {
        // A username with newline could inject additional pf rules
        let err = generate_pf_rules("_mino\npass out quick", "sess-1", None).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn pf_rules_valid_usernames_accepted() {
        // Standard macOS system usernames with underscores and hyphens
        let rules = generate_pf_rules("_mino_agent", "sess-1", None).unwrap();
        assert!(rules.contains("user _mino_agent"));

        let rules = generate_pf_rules("sandbox-user", "sess-1", None).unwrap();
        assert!(rules.contains("user sandbox-user"));
    }

    #[test]
    fn pf_rules_session_id_not_in_output() {
        // Session ID is currently unused in rule generation (_session_id param)
        // to avoid injection. Verify it doesn't leak into rules.
        let rules = generate_pf_rules("_mino_agent", "'; DROP TABLE users;--", None).unwrap();
        assert!(!rules.contains("DROP TABLE"));
        assert!(!rules.contains("';"));
    }

    #[test]
    fn pf_rules_proxy_port_boundary_values() {
        // Port 0
        let rules = generate_pf_rules("_mino_agent", "sess-1", Some(0)).unwrap();
        assert!(rules.contains("port 0 user _mino_agent"));

        // Port max
        let rules = generate_pf_rules("_mino_agent", "sess-1", Some(65535)).unwrap();
        assert!(rules.contains("port 65535 user _mino_agent"));
    }

    #[test]
    fn pf_rules_pass_rules_before_block_rule() {
        // pf evaluates rules in order; `quick` stops on first match.
        // Pass rules MUST come before the block rule or they are unreachable.
        let rules = generate_pf_rules("_mino_agent", "sess-1", Some(8080)).unwrap();
        let block_pos = rules.find("block out quick").expect("missing block rule");
        let dns_pos = rules
            .find("pass out quick proto udp to any port 53")
            .expect("missing DNS pass");
        let proxy_pos = rules
            .find("pass out quick proto tcp to 127.0.0.1 port 8080")
            .expect("missing proxy pass");
        assert!(dns_pos < block_pos, "DNS pass must come before block");
        assert!(proxy_pos < block_pos, "proxy pass must come before block");
    }
}
