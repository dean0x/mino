//! Mino sandbox helper - macOS privileged helper binary
//!
//! Installed to /usr/local/bin/ during `mino setup --native`.
//! Called via sudoers.d: `<user> ALL=(root) NOPASSWD: /usr/local/bin/mino-sandbox-helper`
//!
//! Operations:
//! - spawn: Set ACLs, create pf sub-anchor, fork+setuid to sandbox user, exec command
//! - exec: Drop privileges to sandbox user and exec a command (no ACL setup, no fork)
//! - cleanup: Remove ACLs, remove pf sub-anchor
//! - health-check: Return version

use std::process;

use mino::sandbox::helper_protocol::{HelperRequest, HelperResponse};

mod acl;
mod cleanup;
mod dotfiles;
mod exec;
mod lifecycle;
mod resource_limits;
mod signal;

use cleanup::dispatch_cleanup;
use exec::handle_exec;
use lifecycle::{handle_spawn, SpawnParams};

/// Load a HelperRequest from the --request-file argument.
///
/// Reads and parses the JSON file, then deletes it immediately to minimize
/// the time credentials sit on disk.
pub(crate) fn load_request(args: &[String]) -> Result<HelperRequest, String> {
    let request_file = args
        .iter()
        .position(|a| a == "--request-file")
        .and_then(|i| args.get(i + 1))
        .ok_or_else(|| "Missing --request-file argument".to_string())?;
    let content = std::fs::read_to_string(request_file)
        .map_err(|e| format!("Failed to read request file '{}': {}", request_file, e))?;
    // Delete credential file immediately after reading — shortest time on disk
    std::fs::remove_file(request_file).ok();
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse request: {}", e))
}

fn respond_healthy() -> Result<i32, String> {
    print_response(&HelperResponse::Healthy {
        version: env!("CARGO_PKG_VERSION").to_string(),
    });
    Ok(0)
}

fn print_response(response: &HelperResponse) {
    if let Ok(json) = serde_json::to_string(response) {
        println!("{}", json);
    }
}

pub(crate) fn print_error(message: &str) {
    eprintln!("[mino-helper] {}", message);
}

/// Convert a slice of &str into Vec<String>.
///
/// Available to all test modules in this crate via `crate::str_args`.
#[cfg(test)]
pub(crate) fn str_args(slice: &[&str]) -> Vec<String> {
    slice.iter().map(|s| s.to_string()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 2 && args[1] == "--version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if args.len() < 2 {
        eprintln!("Usage: mino-sandbox-helper <action> --request-file <path>");
        eprintln!("       mino-sandbox-helper --version");
        process::exit(1);
    }

    let action = &args[1];

    let result: Result<i32, String> = match action.as_str() {
        "spawn" => {
            let request = match load_request(&args) {
                Ok(r) => r,
                Err(msg) => {
                    print_error(&msg);
                    process::exit(1);
                }
            };

            SpawnParams::try_from(request).and_then(handle_spawn)
        }
        "cleanup" => dispatch_cleanup(&args),
        "exec" => handle_exec(&args[2..]),
        "health-check" => respond_healthy(),
        _ => {
            print_error(&format!("Unknown action: {}", action));
            process::exit(1);
        }
    };

    match result {
        Ok(code) => process::exit(code),
        Err(msg) => {
            print_error(&msg);
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mino::sandbox::helper_protocol::ResourceLimitsDto;
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::str_args as args;

    // ---- load_request tests ----

    #[test]
    fn load_request_spawn_from_file() {
        use mino::sandbox::helper_protocol::AclEntry;

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("request.json");
        let json = serde_json::to_string(&HelperRequest::Spawn {
            session_id: "test-sess".to_string(),
            project_dir: PathBuf::from("/tmp/project"),
            env: HashMap::from([("FOO".to_string(), "bar".to_string())]),
            command: vec!["echo".to_string(), "hello".to_string()],
            resource_limits: ResourceLimitsDto {
                max_memory_bytes: 1024,
                max_processes: 64,
                max_cpu_seconds: 300,
                max_file_size_bytes: 1048576,
            },
            acl_paths: vec![AclEntry {
                path: PathBuf::from("/tmp/project"),
                writable: true,
            }],
            dotfile_dir: None,
            home_dir: PathBuf::from("/tmp/mino-home-test-sess"),
            sandbox_user: "_mino_agent".to_string(),
        })
        .unwrap();
        std::fs::write(&file_path, &json).unwrap();

        let cli_args = args(&[
            "mino-sandbox-helper",
            "spawn",
            "--request-file",
            file_path.to_str().unwrap(),
        ]);
        let request = load_request(&cli_args).unwrap();

        match request {
            HelperRequest::Spawn {
                session_id,
                command,
                env,
                sandbox_user,
                ..
            } => {
                assert_eq!(session_id, "test-sess");
                assert_eq!(command, vec!["echo", "hello"]);
                assert_eq!(env.get("FOO").unwrap(), "bar");
                assert_eq!(sandbox_user, "_mino_agent");
            }
            _ => panic!("expected Spawn variant"),
        }

        // File should be deleted after loading
        assert!(
            !file_path.exists(),
            "request file should be deleted after loading"
        );
    }

    #[test]
    fn load_request_cleanup_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("cleanup.json");
        let json = serde_json::to_string(&HelperRequest::Cleanup {
            session_id: "sess-cleanup".to_string(),
            project_dir: PathBuf::from("/home/user/proj"),
            sandbox_user: "_mino_agent".to_string(),
        })
        .unwrap();
        std::fs::write(&file_path, &json).unwrap();

        let cli_args = args(&[
            "mino-sandbox-helper",
            "cleanup",
            "--request-file",
            file_path.to_str().unwrap(),
        ]);
        let request = load_request(&cli_args).unwrap();

        match request {
            HelperRequest::Cleanup {
                session_id,
                project_dir,
                sandbox_user,
            } => {
                assert_eq!(session_id, "sess-cleanup");
                assert_eq!(project_dir, PathBuf::from("/home/user/proj"));
                assert_eq!(sandbox_user, "_mino_agent");
            }
            _ => panic!("expected Cleanup variant"),
        }
    }

    #[test]
    fn load_request_missing_flag() {
        let cli_args = args(&["mino-sandbox-helper", "spawn"]);
        let err = load_request(&cli_args).unwrap_err();
        assert!(
            err.contains("--request-file"),
            "expected error about --request-file, got: {}",
            err
        );
    }

    #[test]
    fn load_request_nonexistent_file() {
        let cli_args = args(&[
            "mino-sandbox-helper",
            "spawn",
            "--request-file",
            "/tmp/does-not-exist-mino-test.json",
        ]);
        let err = load_request(&cli_args).unwrap_err();
        assert!(
            err.contains("Failed to read"),
            "expected read error, got: {}",
            err
        );
    }

    #[test]
    fn load_request_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bad.json");
        std::fs::write(&file_path, "not valid json {{{").unwrap();

        let cli_args = args(&[
            "mino-sandbox-helper",
            "spawn",
            "--request-file",
            file_path.to_str().unwrap(),
        ]);
        let err = load_request(&cli_args).unwrap_err();
        assert!(
            err.contains("Failed to parse"),
            "expected parse error, got: {}",
            err
        );
    }

    #[test]
    fn load_request_health_check_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("health.json");
        let json = serde_json::to_string(&HelperRequest::HealthCheck).unwrap();
        std::fs::write(&file_path, &json).unwrap();

        let cli_args = args(&[
            "mino-sandbox-helper",
            "health-check",
            "--request-file",
            file_path.to_str().unwrap(),
        ]);
        let request = load_request(&cli_args).unwrap();
        assert!(matches!(request, HelperRequest::HealthCheck));
    }
}
