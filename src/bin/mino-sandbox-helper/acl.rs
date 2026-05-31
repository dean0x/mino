use std::path::Path;

use mino::sandbox::helper;
use mino::sandbox::helper_protocol::AclEntry;

/// Set an ACL on a path granting the sandbox user access.
pub(crate) fn set_acl(path: &Path, writable: bool, sandbox_user: &str) -> Result<(), String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| "Invalid UTF-8 in path".to_string())?;

    let args = helper::build_acl_args(path_str, sandbox_user, writable);

    let output = std::process::Command::new("chmod")
        .args(&args)
        .output()
        .map_err(|e| format!("chmod +a failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("chmod +a failed: {}", stderr));
    }

    Ok(())
}

/// Remove ACLs from all tracked paths and optionally the home directory.
pub(crate) fn cleanup_acls(acl_paths: &[AclEntry], home_dir: Option<&Path>, sandbox_user: &str) {
    for acl in acl_paths {
        let _ = remove_acl(&acl.path, acl.writable, sandbox_user);
    }
    if let Some(home) = home_dir {
        // Home dir ACL is always writable=true (set in step 2 of handle_spawn)
        let _ = remove_acl(home, true, sandbox_user);
    }
}

/// Remove a single ACL entry from a path.
///
/// `writable` must match what was originally set: each path gets either a
/// read-write OR a read-only ACL, never both. Calling `chmod -a` with the
/// wrong variant is a no-op, so this is safe — but passing the correct flag
/// avoids a redundant `-RH` directory walk on large trees (.nvm, .oh-my-zsh).
pub(crate) fn remove_acl(path: &Path, writable: bool, sandbox_user: &str) -> Result<(), String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("Path contains invalid UTF-8: {:?}", path))?;

    let args = helper::build_remove_acl_args(path_str, sandbox_user, writable);
    let _ = std::process::Command::new("chmod").args(&args).output();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mino::sandbox::helper_protocol::AclEntry;
    use std::path::PathBuf;

    // ---- set_acl path validation ----

    #[cfg(unix)]
    #[test]
    fn set_acl_rejects_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        // Construct a path with a non-UTF-8 byte sequence.
        let bad_bytes: &[u8] = b"/tmp/bad\xff\xfe";
        let bad_os = std::ffi::OsStr::from_bytes(bad_bytes);
        let path = PathBuf::from(bad_os);
        let result = set_acl(&path, false, "_mino_agent");
        assert!(result.is_err(), "non-UTF-8 path must return Err");
        assert!(
            result.unwrap_err().contains("Invalid UTF-8"),
            "error should mention UTF-8"
        );
    }

    #[test]
    fn set_acl_valid_path_returns_ok_or_chmod_error() {
        // With a valid UTF-8 path the function proceeds to spawn chmod.
        // In a test environment chmod either succeeds or fails (e.g. no such
        // file), but it must never return the UTF-8 validation error.
        let result = set_acl(Path::new("/tmp"), false, "_mino_agent");
        // The result depends on the OS/environment, but the error must not be
        // the UTF-8 message.
        if let Err(e) = result {
            assert!(
                !e.contains("Invalid UTF-8"),
                "UTF-8 error must not appear for a valid path"
            );
        }
    }

    // ---- remove_acl path validation ----

    #[cfg(unix)]
    #[test]
    fn remove_acl_rejects_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad_bytes: &[u8] = b"/tmp/bad\xff";
        let bad_os = std::ffi::OsStr::from_bytes(bad_bytes);
        let path = PathBuf::from(bad_os);
        let result = remove_acl(&path, true, "_mino_agent");
        assert!(result.is_err(), "non-UTF-8 path must return Err");
        assert!(result.unwrap_err().contains("invalid UTF-8"));
    }

    // ---- cleanup_acls iteration ----

    #[test]
    fn cleanup_acls_processes_all_entries() {
        // cleanup_acls is best-effort (ignores errors) and processes every
        // entry in the list plus an optional home_dir.  We can't assert on
        // side-effects without running as root, but we can verify the function
        // does not panic when given multiple entries and a home_dir.
        let entries = vec![
            AclEntry {
                path: PathBuf::from("/tmp"),
                writable: false,
            },
            AclEntry {
                path: PathBuf::from("/tmp"),
                writable: true,
            },
        ];
        let home = PathBuf::from("/tmp");
        // Should not panic regardless of chmod outcome.
        cleanup_acls(&entries, Some(&home), "_mino_agent");
    }

    #[test]
    fn cleanup_acls_no_home_is_ok() {
        let entries = vec![AclEntry {
            path: PathBuf::from("/tmp"),
            writable: false,
        }];
        // home_dir = None must not panic.
        cleanup_acls(&entries, None, "_mino_agent");
    }

    #[test]
    fn cleanup_acls_empty_list_no_home() {
        // Degenerate case: empty list, no home.
        cleanup_acls(&[], None, "_mino_agent");
    }
}
