//! Pure helper functions shared between mino CLI and mino-sandbox-helper
//!
//! These are testable pure functions that generate arguments for system commands
//! (chmod, pfctl) without actually executing them. The helper binary imports
//! these to avoid duplicating logic.

use crate::error::MinoResult;
use crate::session::validate_session_name;
use std::collections::HashMap;
use std::path::Path;

/// ACL permission string for macOS `chmod +a/-a` commands.
fn acl_perms(writable: bool) -> &'static str {
    if writable {
        "allow read,write,execute,file_inherit,directory_inherit"
    } else {
        "allow read,execute,file_inherit,directory_inherit"
    }
}

/// Generate `chmod +a` arguments to add an ACL entry for a user.
///
/// Returns the full argument list for `std::process::Command::new("chmod").args(...)`.
pub fn build_acl_args(path: &str, username: &str, writable: bool) -> Vec<String> {
    vec![
        "+a".to_string(),
        format!("{} {}", username, acl_perms(writable)),
        path.to_string(),
    ]
}

/// Generate `chmod -a` arguments to remove an ACL entry for a user.
///
/// Returns the full argument list for `std::process::Command::new("chmod").args(...)`.
pub fn build_remove_acl_args(path: &str, username: &str, writable: bool) -> Vec<String> {
    vec![
        "-a".to_string(),
        format!("{} {}", username, acl_perms(writable)),
        path.to_string(),
    ]
}

/// Generate `pfctl` arguments to flush pf rules for a session's sub-anchor.
///
/// Validates the session_id using `validate_session_name` to prevent anchor
/// path injection. Returns the argument list for `pfctl`.
pub fn build_pf_cleanup_args(session_id: &str) -> MinoResult<Vec<String>> {
    validate_session_name(session_id)?;
    Ok(vec![
        "-a".to_string(),
        format!("mino/session-{}", session_id),
        "-F".to_string(),
        "rules".to_string(),
    ])
}

/// Build the full environment for a child process inside the sandbox.
///
/// Starts with `base_env` (the env vars from the request), then overrides
/// HOME and USER to point at the sandbox home directory and user.
pub fn build_child_env(
    base_env: &HashMap<String, String>,
    home_dir: &Path,
    sandbox_user: &str,
) -> Result<HashMap<String, String>, String> {
    let home = home_dir
        .to_str()
        .ok_or_else(|| format!("Home directory path contains invalid UTF-8: {:?}", home_dir))?;
    let mut env = base_env.clone();
    env.insert("HOME".to_string(), home.to_string());
    env.insert("USER".to_string(), sandbox_user.to_string());
    Ok(env)
}

/// Parse UID and GID from combined dscl output.
///
/// Expects output format from `dscl . -read /Users/<name> UniqueID PrimaryGroupID`:
/// ```text
/// UniqueID: 502
/// PrimaryGroupID: 20
/// ```
pub fn parse_dscl_ids(output: &str) -> Result<(u32, u32), String> {
    let mut uid: Option<u32> = None;
    let mut gid: Option<u32> = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("UniqueID:") {
            uid = Some(
                value
                    .trim()
                    .parse()
                    .map_err(|_| format!("Non-numeric UniqueID: {}", value.trim()))?,
            );
        } else if let Some(value) = line.strip_prefix("PrimaryGroupID:") {
            gid = Some(
                value
                    .trim()
                    .parse()
                    .map_err(|_| format!("Non-numeric PrimaryGroupID: {}", value.trim()))?,
            );
        }
    }

    let uid = uid.ok_or_else(|| "Missing UniqueID in dscl output".to_string())?;
    let gid = gid.ok_or_else(|| "Missing PrimaryGroupID in dscl output".to_string())?;
    Ok((uid, gid))
}

/// Build a minimal environment for exec into an existing sandbox session.
///
/// Unlike `build_child_env`, this does not inherit the original request env.
/// Instead it provides only the essentials: HOME, USER, PATH, and TERM
/// (from the host process if available).
pub fn build_exec_env(home_dir: &Path, sandbox_user: &str) -> Result<HashMap<String, String>, String> {
    let home = home_dir
        .to_str()
        .ok_or_else(|| format!("Home directory path contains invalid UTF-8: {:?}", home_dir))?;
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), home.to_string());
    env.insert("USER".to_string(), sandbox_user.to_string());
    env.insert(
        "PATH".to_string(),
        "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
    );
    if let Ok(term) = std::env::var("TERM") {
        env.insert("TERM".to_string(), term);
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- build_acl_args tests ----

    #[test]
    fn acl_args_readonly() {
        let args = build_acl_args("/tmp/project", "_mino_agent", false);
        assert_eq!(args[0], "+a");
        assert!(args[1].contains("_mino_agent"));
        assert!(args[1].contains("read,execute"));
        assert!(!args[1].contains("write"));
        assert_eq!(args[2], "/tmp/project");
    }

    #[test]
    fn acl_args_writable() {
        let args = build_acl_args("/tmp/project", "_mino_agent", true);
        assert_eq!(args[0], "+a");
        assert!(args[1].contains("read,write,execute"));
        assert_eq!(args[2], "/tmp/project");
    }

    #[test]
    fn acl_args_custom_username() {
        let args = build_acl_args("/tmp/p", "custom-user", false);
        assert!(args[1].starts_with("custom-user "));
    }

    // ---- build_remove_acl_args tests ----

    #[test]
    fn remove_acl_args_readonly() {
        let args = build_remove_acl_args("/tmp/project", "_mino_agent", false);
        assert_eq!(args[0], "-a");
        assert!(args[1].contains("read,execute"));
        assert!(!args[1].contains("write"));
    }

    #[test]
    fn remove_acl_args_writable() {
        let args = build_remove_acl_args("/tmp/project", "_mino_agent", true);
        assert_eq!(args[0], "-a");
        assert!(args[1].contains("read,write,execute"));
    }

    // ---- build_pf_cleanup_args tests ----

    #[test]
    fn pf_cleanup_args_valid_id() {
        let args = build_pf_cleanup_args("test-session-123").unwrap();
        assert_eq!(args[0], "-a");
        assert_eq!(args[1], "mino/session-test-session-123");
        assert_eq!(args[2], "-F");
        assert_eq!(args[3], "rules");
    }

    #[test]
    fn pf_cleanup_args_rejects_slash() {
        let result = build_pf_cleanup_args("bad/path");
        assert!(result.is_err());
    }

    #[test]
    fn pf_cleanup_args_rejects_dotdot() {
        let result = build_pf_cleanup_args("..");
        assert!(result.is_err());
    }

    #[test]
    fn pf_cleanup_args_rejects_empty() {
        let result = build_pf_cleanup_args("");
        assert!(result.is_err());
    }

    // ---- build_child_env tests ----

    #[test]
    fn child_env_sets_home_and_user() {
        let base = HashMap::from([("KEY".to_string(), "val".to_string())]);
        let env = build_child_env(&base, Path::new("/home/sandbox"), "_mino_agent").unwrap();
        assert_eq!(env.get("HOME").unwrap(), "/home/sandbox");
        assert_eq!(env.get("USER").unwrap(), "_mino_agent");
    }

    #[test]
    fn child_env_preserves_base() {
        let base = HashMap::from([("CUSTOM".to_string(), "value".to_string())]);
        let env = build_child_env(&base, Path::new("/tmp"), "agent").unwrap();
        assert_eq!(env.get("CUSTOM").unwrap(), "value");
    }

    #[test]
    fn child_env_sandbox_user_in_user() {
        let env = build_child_env(&HashMap::new(), Path::new("/tmp"), "my-user").unwrap();
        assert_eq!(env.get("USER").unwrap(), "my-user");
    }

    // ---- parse_dscl_ids tests ----

    #[test]
    fn parse_dscl_ids_valid() {
        let output = "UniqueID: 502\nPrimaryGroupID: 20\n";
        let (uid, gid) = parse_dscl_ids(output).unwrap();
        assert_eq!(uid, 502);
        assert_eq!(gid, 20);
    }

    #[test]
    fn parse_dscl_ids_reversed_order() {
        let output = "PrimaryGroupID: 20\nUniqueID: 502\n";
        let (uid, gid) = parse_dscl_ids(output).unwrap();
        assert_eq!(uid, 502);
        assert_eq!(gid, 20);
    }

    #[test]
    fn parse_dscl_ids_missing_uid() {
        let output = "PrimaryGroupID: 20\n";
        let err = parse_dscl_ids(output).unwrap_err();
        assert!(err.contains("Missing UniqueID"));
    }

    #[test]
    fn parse_dscl_ids_missing_gid() {
        let output = "UniqueID: 502\n";
        let err = parse_dscl_ids(output).unwrap_err();
        assert!(err.contains("Missing PrimaryGroupID"));
    }

    #[test]
    fn parse_dscl_ids_non_numeric_uid() {
        let output = "UniqueID: abc\nPrimaryGroupID: 20\n";
        let err = parse_dscl_ids(output).unwrap_err();
        assert!(err.contains("Non-numeric UniqueID"));
    }

    #[test]
    fn parse_dscl_ids_non_numeric_gid() {
        let output = "UniqueID: 502\nPrimaryGroupID: xyz\n";
        let err = parse_dscl_ids(output).unwrap_err();
        assert!(err.contains("Non-numeric PrimaryGroupID"));
    }

    #[test]
    fn parse_dscl_ids_empty_output() {
        let err = parse_dscl_ids("").unwrap_err();
        assert!(err.contains("Missing UniqueID"));
    }

    #[test]
    fn parse_dscl_ids_extra_whitespace() {
        let output = "UniqueID:   502  \n  PrimaryGroupID:   20  \n";
        let (uid, gid) = parse_dscl_ids(output).unwrap();
        assert_eq!(uid, 502);
        assert_eq!(gid, 20);
    }

    // ---- build_exec_env tests ----

    #[test]
    fn exec_env_has_home_user_path() {
        let env = build_exec_env(&PathBuf::from("/home/agent"), "_mino_agent").unwrap();
        assert_eq!(env.get("HOME").unwrap(), "/home/agent");
        assert_eq!(env.get("USER").unwrap(), "_mino_agent");
        assert!(env.get("PATH").unwrap().contains("/usr/bin"));
    }

    #[test]
    fn exec_env_minimal_keys() {
        let env = build_exec_env(&PathBuf::from("/tmp"), "agent").unwrap();
        // Should have HOME, USER, PATH, and optionally TERM
        assert!(env.contains_key("HOME"));
        assert!(env.contains_key("USER"));
        assert!(env.contains_key("PATH"));
        // TERM may or may not be present depending on test env
        assert!(env.len() <= 4);
    }
}
