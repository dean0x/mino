//! Helper protocol types shared between mino CLI and mino-sandbox-helper
//!
//! Communication: mino writes a JSON request to a temp file, passes the path
//! as a CLI arg.
//!
//! Response protocol by action:
//! - `spawn`: helper exits 0 on success, non-zero on failure. No JSON emitted.
//! - `cleanup`: helper exits 0 on success, non-zero on failure. No JSON emitted.
//! - `health-check`: helper writes a JSON `HelperResponse::Healthy` to stdout.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Action the helper should perform
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HelperRequest {
    /// Spawn a sandboxed process as the sandbox user
    Spawn {
        session_id: String,
        project_dir: PathBuf,
        env: HashMap<String, String>,
        command: Vec<String>,
        resource_limits: ResourceLimitsDto,
        acl_paths: Vec<AclEntry>,
        dotfile_dir: Option<PathBuf>,
        home_dir: PathBuf,
        /// Sandbox user to run as (e.g., "_mino_agent")
        sandbox_user: String,
    },
    /// Clean up ACLs and pf rules for a session
    Cleanup {
        session_id: String,
        project_dir: PathBuf,
        /// Sandbox user whose ACLs to remove (defaults to "_mino_agent" if absent)
        #[serde(default = "default_sandbox_user")]
        sandbox_user: String,
    },
    /// Health check
    HealthCheck,
}

/// Default sandbox user for backward compatibility with older Cleanup requests.
fn default_sandbox_user() -> String {
    super::config::DEFAULT_SANDBOX_USER.to_string()
}

/// ACL entry for a path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclEntry {
    pub path: PathBuf,
    pub writable: bool,
}

/// Resource limits DTO (Data Transfer Object)
///
/// Serializable copy of ResourceLimits for IPC between mino and the helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimitsDto {
    pub max_memory_bytes: u64,
    pub max_processes: u32,
    pub max_cpu_seconds: u64,
    pub max_file_size_bytes: u64,
}

impl From<&crate::sandbox::resource_limits::ResourceLimits> for ResourceLimitsDto {
    fn from(limits: &crate::sandbox::resource_limits::ResourceLimits) -> Self {
        Self {
            max_memory_bytes: limits.max_memory_bytes,
            max_processes: limits.max_processes,
            max_cpu_seconds: limits.max_cpu_seconds,
            max_file_size_bytes: limits.max_file_size_bytes,
        }
    }
}

/// Response from the helper
///
/// Only emitted as JSON on stdout for the `health-check` action.
/// Spawn and cleanup communicate success/failure via exit code only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum HelperResponse {
    /// Health check passed — the only variant emitted as JSON to stdout
    Healthy { version: String },
    /// Error occurred
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_request_roundtrip() {
        let request = HelperRequest::Spawn {
            session_id: "sess-123".to_string(),
            project_dir: PathBuf::from("/home/user/project"),
            env: HashMap::from([("PATH".to_string(), "/usr/bin".to_string())]),
            command: vec!["bash".to_string(), "-c".to_string(), "echo hi".to_string()],
            resource_limits: ResourceLimitsDto {
                max_memory_bytes: 4_294_967_296,
                max_processes: 256,
                max_cpu_seconds: 3600,
                max_file_size_bytes: 104_857_600,
            },
            acl_paths: vec![
                AclEntry {
                    path: PathBuf::from("/home/user/project"),
                    writable: true,
                },
                AclEntry {
                    path: PathBuf::from("/usr/share/data"),
                    writable: false,
                },
            ],
            dotfile_dir: Some(PathBuf::from("/tmp/dotfiles")),
            home_dir: PathBuf::from("/tmp/mino-home-sess-123"),
            sandbox_user: "_mino_agent".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: HelperRequest = serde_json::from_str(&json).unwrap();

        match parsed {
            HelperRequest::Spawn {
                session_id,
                project_dir,
                env,
                command,
                resource_limits,
                acl_paths,
                dotfile_dir,
                home_dir,
                sandbox_user,
            } => {
                assert_eq!(session_id, "sess-123");
                assert_eq!(project_dir, PathBuf::from("/home/user/project"));
                assert_eq!(env.get("PATH").unwrap(), "/usr/bin");
                assert_eq!(command, vec!["bash", "-c", "echo hi"]);
                assert_eq!(resource_limits.max_memory_bytes, 4_294_967_296);
                assert_eq!(resource_limits.max_processes, 256);
                assert_eq!(acl_paths.len(), 2);
                assert!(acl_paths[0].writable);
                assert!(!acl_paths[1].writable);
                assert_eq!(dotfile_dir, Some(PathBuf::from("/tmp/dotfiles")));
                assert_eq!(home_dir, PathBuf::from("/tmp/mino-home-sess-123"));
                assert_eq!(sandbox_user, "_mino_agent");
            }
            _ => panic!("expected Spawn variant"),
        }
    }

    #[test]
    fn cleanup_request_roundtrip() {
        let request = HelperRequest::Cleanup {
            session_id: "sess-456".to_string(),
            project_dir: PathBuf::from("/home/user/project"),
            sandbox_user: "custom-user".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: HelperRequest = serde_json::from_str(&json).unwrap();

        match parsed {
            HelperRequest::Cleanup {
                session_id,
                project_dir,
                sandbox_user,
            } => {
                assert_eq!(session_id, "sess-456");
                assert_eq!(project_dir, PathBuf::from("/home/user/project"));
                assert_eq!(sandbox_user, "custom-user");
            }
            _ => panic!("expected Cleanup variant"),
        }
    }

    #[test]
    fn cleanup_request_backward_compat_default_user() {
        // Old-format JSON without sandbox_user field should default
        let json = r#"{"action":"Cleanup","session_id":"s1","project_dir":"/tmp"}"#;
        let parsed: HelperRequest = serde_json::from_str(json).unwrap();
        match parsed {
            HelperRequest::Cleanup { sandbox_user, .. } => {
                assert_eq!(sandbox_user, "_mino_agent");
            }
            _ => panic!("expected Cleanup variant"),
        }
    }

    #[test]
    fn health_check_request_roundtrip() {
        let request = HelperRequest::HealthCheck;
        let json = serde_json::to_string(&request).unwrap();
        let parsed: HelperRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, HelperRequest::HealthCheck));
    }

    #[test]
    fn healthy_response_roundtrip() {
        let response = HelperResponse::Healthy {
            version: "1.6.0".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: HelperResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            HelperResponse::Healthy { version } => assert_eq!(version, "1.6.0"),
            _ => panic!("expected Healthy variant"),
        }
    }

    #[test]
    fn error_response_roundtrip() {
        let response = HelperResponse::Error {
            message: "something went wrong".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: HelperResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            HelperResponse::Error { message } => assert_eq!(message, "something went wrong"),
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn resource_limits_dto_from_conversion() {
        use crate::sandbox::resource_limits::ResourceLimits;

        let limits = ResourceLimits {
            max_memory_bytes: 4_294_967_296,
            max_processes: 128,
            max_cpu_seconds: 1800,
            max_file_size_bytes: 52_428_800,
        };

        let dto = ResourceLimitsDto::from(&limits);
        assert_eq!(dto.max_memory_bytes, 4_294_967_296);
        assert_eq!(dto.max_processes, 128);
        assert_eq!(dto.max_cpu_seconds, 1800);
        assert_eq!(dto.max_file_size_bytes, 52_428_800);
    }

    #[test]
    fn acl_entry_serializes_correctly() {
        let entry = AclEntry {
            path: PathBuf::from("/home/user/project"),
            writable: true,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("/home/user/project"));
        assert!(json.contains("true"));

        let parsed: AclEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.path, PathBuf::from("/home/user/project"));
        assert!(parsed.writable);
    }

    #[test]
    fn spawn_request_tagged_with_action() {
        let request = HelperRequest::Spawn {
            session_id: "s1".to_string(),
            project_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            command: vec!["true".to_string()],
            resource_limits: ResourceLimitsDto {
                max_memory_bytes: 0,
                max_processes: 0,
                max_cpu_seconds: 0,
                max_file_size_bytes: 0,
            },
            acl_paths: vec![],
            dotfile_dir: None,
            home_dir: PathBuf::from("/tmp/home"),
            sandbox_user: "_mino_agent".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        // Verify the tag discriminant is present
        assert!(json.contains(r#""action":"Spawn"#));
        assert!(json.contains(r#""sandbox_user":"_mino_agent"#));
    }

    #[test]
    fn cleanup_request_tagged_with_action() {
        let request = HelperRequest::Cleanup {
            session_id: "s1".to_string(),
            project_dir: PathBuf::from("/tmp"),
            sandbox_user: "_mino_agent".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""action":"Cleanup"#));
    }

    #[test]
    fn health_check_request_tagged_with_action() {
        let request = HelperRequest::HealthCheck;
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""action":"HealthCheck"#));
    }
}
