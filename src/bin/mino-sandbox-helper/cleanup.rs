use std::path::{Path, PathBuf};

use mino::sandbox::helper;
use mino::session::validate_session_name;

use super::acl::remove_acl;

/// Parsed arguments for the cleanup subcommand.
#[derive(Debug)]
pub(crate) struct CleanupArgs<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) project_dir: PathBuf,
    pub(crate) sandbox_user: &'a str,
}

/// Parse cleanup subcommand arguments into a CleanupArgs struct.
pub(crate) fn parse_cleanup_args(args: &[String]) -> Result<CleanupArgs<'_>, String> {
    let mut session_id: Option<&str> = None;
    let mut project_dir: Option<&str> = None;
    let mut sandbox_user: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session-id" => {
                session_id = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--project-dir" => {
                project_dir = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--sandbox-user" => {
                sandbox_user = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    Ok(CleanupArgs {
        session_id: session_id.ok_or("Missing --session-id")?,
        project_dir: PathBuf::from(project_dir.ok_or("Missing --project-dir")?),
        sandbox_user: sandbox_user.ok_or("Missing --sandbox-user")?,
    })
}

pub(crate) fn handle_cleanup(
    session_id: &str,
    project_dir: &Path,
    sandbox_user: &str,
) -> Result<(), String> {
    // Validate inputs — this binary runs as root so all inputs must be checked
    validate_session_name(session_id).map_err(|e| format!("Invalid session_id: {}", e))?;
    mino::sandbox::config::validate_sandbox_user(sandbox_user)
        .map_err(|e| format!("Invalid sandbox_user: {}", e))?;

    // Remove ACLs on project dir — project dir is always writable=true
    let _ = remove_acl(project_dir, true, sandbox_user);

    // Remove pf sub-anchor using validated args from library
    if let Ok(pf_args) = helper::build_pf_cleanup_args(session_id) {
        let _ = std::process::Command::new("pfctl").args(&pf_args).output();
    }

    // Remove staging home dir (owned by _mino_agent, requires root).
    // Best-effort: log on failure but do not abort cleanup.
    let home_dir = PathBuf::from(format!("/tmp/mino-home-{}", session_id));
    if home_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&home_dir) {
            eprintln!(
                "Warning: failed to remove staging home dir {}: {}",
                home_dir.display(),
                e
            );
        }
    }

    Ok(())
}

/// Dispatch the "cleanup" subcommand.
///
/// Tries CLI args first (preferred), then falls back to the request-file protocol
/// for backward compatibility with callers that still use `--request-file`.
pub(crate) fn dispatch_cleanup(args: &[String]) -> Result<i32, String> {
    use mino::sandbox::helper_protocol::HelperRequest;

    // Primary path: all fields supplied as CLI flags.
    if let Ok(parsed) = parse_cleanup_args(&args[2..]) {
        return handle_cleanup(parsed.session_id, &parsed.project_dir, parsed.sandbox_user)
            .map(|()| 0);
    }

    // Fallback path: fields encoded in a JSON request file (legacy callers).
    let request = super::load_request(args)?;
    match request {
        HelperRequest::Cleanup {
            session_id,
            project_dir,
            sandbox_user,
        } => handle_cleanup(&session_id, &project_dir, &sandbox_user).map(|()| 0),
        _ => Err("Expected Cleanup request".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert a slice of &str into Vec<String> for test convenience.
    fn args(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    // ---- parse_cleanup_args tests ----

    #[test]
    fn parse_cleanup_args_valid() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--project-dir",
            "/home/user/project",
            "--sandbox-user",
            "_mino_agent",
        ]);
        let parsed = parse_cleanup_args(&input).unwrap();
        assert_eq!(parsed.session_id, "my-session");
        assert_eq!(parsed.project_dir, PathBuf::from("/home/user/project"));
        assert_eq!(parsed.sandbox_user, "_mino_agent");
    }

    #[test]
    fn parse_cleanup_args_missing_session_id() {
        let input = args(&[
            "--project-dir",
            "/tmp/proj",
            "--sandbox-user",
            "_mino_agent",
        ]);
        let err = parse_cleanup_args(&input).unwrap_err();
        assert!(
            err.contains("--session-id"),
            "expected error about --session-id, got: {}",
            err
        );
    }

    #[test]
    fn parse_cleanup_args_missing_project_dir() {
        let input = args(&["--session-id", "s1", "--sandbox-user", "_mino_agent"]);
        let err = parse_cleanup_args(&input).unwrap_err();
        assert!(
            err.contains("--project-dir"),
            "expected error about --project-dir, got: {}",
            err
        );
    }

    #[test]
    fn parse_cleanup_args_missing_sandbox_user() {
        let input = args(&["--session-id", "s1", "--project-dir", "/tmp/proj"]);
        let err = parse_cleanup_args(&input).unwrap_err();
        assert!(
            err.contains("--sandbox-user"),
            "expected error about --sandbox-user, got: {}",
            err
        );
    }

    // ---- handle_cleanup staging-dir removal tests ----

    #[test]
    fn handle_cleanup_removes_staging_home_dir() {
        // Create a unique staging dir under /tmp that handle_cleanup should delete.
        // We use a session-name derived from the test's pid to avoid collisions.
        let session_id = format!("cleanup-test-{}", std::process::id());
        let home_dir = PathBuf::from(format!("/tmp/mino-home-{}", session_id));
        std::fs::create_dir_all(&home_dir).unwrap();
        std::fs::write(home_dir.join("some-file"), "data").unwrap();

        // handle_cleanup will fail on pfctl/chmod (not root) but must still
        // remove the staging dir, which we created as the current user.
        let project_dir = tempfile::tempdir().unwrap();
        let _ = handle_cleanup(&session_id, project_dir.path(), "_mino_agent");

        assert!(
            !home_dir.exists(),
            "handle_cleanup must remove /tmp/mino-home-{{session_id}}"
        );
    }

    #[test]
    fn handle_cleanup_tolerates_missing_home_dir() {
        // If the staging dir was already removed (e.g. agent cleaned up on exit),
        // handle_cleanup must not error.
        let session_id = format!("cleanup-absent-{}", std::process::id());
        // Do NOT create /tmp/mino-home-{session_id}
        let project_dir = tempfile::tempdir().unwrap();
        // Should return Ok (dir absence is not an error)
        assert!(
            handle_cleanup(&session_id, project_dir.path(), "_mino_agent").is_ok(),
            "handle_cleanup must return Ok when home dir is absent"
        );
    }
}
