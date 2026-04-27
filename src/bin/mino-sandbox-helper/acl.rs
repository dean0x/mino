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
/// avoids a redundant `-R` directory walk on large trees (.nvm, .oh-my-zsh).
pub(crate) fn remove_acl(path: &Path, writable: bool, sandbox_user: &str) -> Result<(), String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("Path contains invalid UTF-8: {:?}", path))?;

    let args = helper::build_remove_acl_args(path_str, sandbox_user, writable);
    let _ = std::process::Command::new("chmod").args(&args).output();

    Ok(())
}
