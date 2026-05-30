use std::collections::VecDeque;
use std::path::Path;

/// Recreate a single symlink entry in `dst_parent`.
///
/// Reads the symlink target from `src_path` and creates an identical symlink
/// at `dst_parent/<entry-filename>`. Logs and returns `Ok(())` on symlink
/// creation failure so callers continue processing other entries.
///
/// # Errors
/// Returns `Err` only when `read_link` itself fails (i.e. we cannot determine
/// the target). In that case the entry is skipped and an error is logged.
#[cfg(unix)]
pub(crate) fn recreate_symlink_entry(
    src_path: &Path,
    dst_parent: &Path,
    file_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    let target = std::fs::read_link(src_path)?;
    let dst_path = dst_parent.join(file_name);
    if let Err(e) = std::os::unix::fs::symlink(&target, &dst_path) {
        eprintln!(
            "[mino-helper] failed to create symlink {} -> {}: {}",
            dst_path.display(),
            target.display(),
            e
        );
    }
    Ok(())
}

/// Maximum directory depth for [`copy_dotfiles`].
///
/// Staging directories created by the mino CLI are shallow by design, but the
/// helper runs as root and must defend against unexpectedly deep trees that
/// could exhaust the stack or consume unbounded time.
const MAX_COPY_DEPTH: usize = 32;

/// Copy a staging dotfile tree into `dest` using an iterative BFS worklist.
///
/// Uses the same iterative pattern as `src/sandbox/fs_copy.rs` to avoid
/// unbounded recursion. Symlinks are recreated (not followed) matching the
/// behavior described in [`recreate_symlink_entry`]. Directories nested beyond
/// [`MAX_COPY_DEPTH`] are skipped with a warning.
pub(crate) fn copy_dotfiles(src: &Path, dest: &Path) {
    // Worklist entries carry (src_dir, dest_dir, depth).
    let mut queue: VecDeque<(std::path::PathBuf, std::path::PathBuf, usize)> =
        VecDeque::from([(src.to_path_buf(), dest.to_path_buf(), 0)]);

    while let Some((src_dir, dest_dir, depth)) = queue.pop_front() {
        let entries = match std::fs::read_dir(&src_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let src_path = entry.path();
            let file_name = entry.file_name();
            let dest_path = dest_dir.join(&file_name);

            // Use symlink_metadata() (not metadata()) to detect symlinks without
            // following them. Symlinks are recreated, not dereferenced.
            let metadata = match std::fs::symlink_metadata(&src_path) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!(
                        "[mino-helper] skipping dotfile (metadata error): {}: {}",
                        src_path.display(),
                        e
                    );
                    continue;
                }
            };

            if metadata.file_type().is_symlink() {
                // Recreate symlinks from the staging dir — these are created by the
                // mino CLI to bridge host directories (e.g., ~/.oh-my-zsh → /Users/X/.oh-my-zsh).
                // The staging dir is 0700 and CLI-controlled, so these are trusted.
                #[cfg(unix)]
                if let Err(e) = recreate_symlink_entry(&src_path, &dest_dir, &file_name) {
                    eprintln!(
                        "[mino-helper] failed to read symlink {}: {}",
                        src_path.display(),
                        e
                    );
                }
                continue;
            }

            if metadata.is_dir() {
                let next_depth = depth + 1;
                if next_depth > MAX_COPY_DEPTH {
                    eprintln!(
                        "[mino-helper] skipping deeply nested dir (depth {}): {}",
                        next_depth,
                        src_path.display()
                    );
                    continue;
                }
                if let Err(e) = std::fs::create_dir_all(&dest_path) {
                    eprintln!(
                        "[mino-helper] failed to create dir {}: {}",
                        dest_path.display(),
                        e
                    );
                    continue;
                }
                queue.push_back((src_path, dest_path, next_depth));
            } else if let Err(e) = std::fs::copy(&src_path, &dest_path) {
                eprintln!(
                    "[mino-helper] failed to copy dotfile {} -> {}: {}",
                    src_path.display(),
                    dest_path.display(),
                    e
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- recreate_symlink_entry tests ----

    #[cfg(unix)]
    #[test]
    fn recreate_symlink_entry_valid_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        let dst_dir = dir.path().join("dst");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();

        // Create a valid symlink in src
        std::os::unix::fs::symlink("/usr/share/doc", src_dir.join("link")).unwrap();

        let file_name = std::ffi::OsStr::new("link");
        recreate_symlink_entry(&src_dir.join("link"), &dst_dir, file_name).unwrap();

        // Destination should have a symlink with the same target
        let dst_link = dst_dir.join("link");
        let meta = std::fs::symlink_metadata(&dst_link).unwrap();
        assert!(meta.file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(&dst_link).unwrap(),
            std::path::PathBuf::from("/usr/share/doc")
        );
    }

    #[cfg(unix)]
    #[test]
    fn recreate_symlink_entry_dangling_symlink() {
        // A dangling symlink (target doesn't exist) should still be recreated
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        let dst_dir = dir.path().join("dst");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();

        let dangling_target = dir.path().join("nonexistent");
        std::os::unix::fs::symlink(&dangling_target, src_dir.join("dangling")).unwrap();

        let file_name = std::ffi::OsStr::new("dangling");
        recreate_symlink_entry(&src_dir.join("dangling"), &dst_dir, file_name).unwrap();

        let dst_link = dst_dir.join("dangling");
        let meta = std::fs::symlink_metadata(&dst_link).unwrap();
        assert!(meta.file_type().is_symlink());
    }

    // ---- copy_dotfiles tests ----

    #[test]
    fn copy_dotfiles_copies_regular_files() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Create regular files in source
        std::fs::write(src.path().join(".bashrc"), "# bashrc content").unwrap();
        std::fs::write(src.path().join(".profile"), "# profile content").unwrap();

        copy_dotfiles(src.path(), dest.path());

        assert_eq!(
            std::fs::read_to_string(dest.path().join(".bashrc")).unwrap(),
            "# bashrc content"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join(".profile")).unwrap(),
            "# profile content"
        );
    }

    #[test]
    fn copy_dotfiles_recreates_symlinks() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Create a regular file and a symlink
        std::fs::write(src.path().join("regular.txt"), "real file").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/usr/share/data", src.path().join("data-link")).unwrap();
        }

        copy_dotfiles(src.path(), dest.path());

        // Regular file should be copied
        assert!(dest.path().join("regular.txt").exists());

        // Symlink should be recreated pointing to the same target
        #[cfg(unix)]
        {
            let dest_link = dest.path().join("data-link");
            let meta = std::fs::symlink_metadata(&dest_link).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "should be recreated as symlink"
            );
            assert_eq!(
                std::fs::read_link(&dest_link).unwrap(),
                PathBuf::from("/usr/share/data")
            );
        }
    }

    #[test]
    fn copy_dotfiles_recurses_into_directories() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Create nested directory structure
        std::fs::create_dir_all(src.path().join(".config").join("nvim")).unwrap();
        std::fs::write(
            src.path().join(".config").join("nvim").join("init.lua"),
            "-- nvim config",
        )
        .unwrap();
        std::fs::write(
            src.path().join(".config").join("starship.toml"),
            "# starship",
        )
        .unwrap();

        copy_dotfiles(src.path(), dest.path());

        assert_eq!(
            std::fs::read_to_string(dest.path().join(".config").join("nvim").join("init.lua"))
                .unwrap(),
            "-- nvim config"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join(".config").join("starship.toml")).unwrap(),
            "# starship"
        );
    }

    #[test]
    fn copy_dotfiles_empty_source_is_noop() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Empty source directory -- should not error
        copy_dotfiles(src.path(), dest.path());

        // Dest should still be empty (only the dir itself)
        let entries: Vec<_> = std::fs::read_dir(dest.path()).unwrap().collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn copy_dotfiles_nonexistent_source_is_noop() {
        let dest = tempfile::tempdir().unwrap();
        let nonexistent = PathBuf::from("/tmp/mino-test-nonexistent-dir-12345");

        // Should not panic or error -- the function silently handles this
        copy_dotfiles(&nonexistent, dest.path());
    }

    #[test]
    fn copy_dotfiles_mixed_entries() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Regular file
        std::fs::write(src.path().join(".gitconfig"), "[user]\n  name = Test").unwrap();

        // Directory with content
        std::fs::create_dir(src.path().join(".ssh")).unwrap();
        std::fs::write(
            src.path().join(".ssh").join("config"),
            "Host *\n  AddKeysToAgent yes",
        )
        .unwrap();

        // Symlink (should be recreated as symlink, not followed)
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", src.path().join(".hosts-link")).unwrap();

        copy_dotfiles(src.path(), dest.path());

        // Regular file copied
        assert!(dest.path().join(".gitconfig").exists());
        assert_eq!(
            std::fs::read_to_string(dest.path().join(".gitconfig")).unwrap(),
            "[user]\n  name = Test"
        );

        // Directory and its content copied
        assert!(dest.path().join(".ssh").join("config").exists());

        // Symlink recreated as symlink pointing to original target
        #[cfg(unix)]
        {
            let link = dest.path().join(".hosts-link");
            let meta = std::fs::symlink_metadata(&link).unwrap();
            assert!(meta.file_type().is_symlink());
            assert_eq!(
                std::fs::read_link(&link).unwrap(),
                PathBuf::from("/etc/hosts")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_dotfiles_recreates_symlink_in_nested_dir() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Create a directory with a symlink inside it
        std::fs::create_dir(src.path().join("subdir")).unwrap();
        std::fs::write(src.path().join("subdir").join("real.txt"), "content").unwrap();
        std::os::unix::fs::symlink("/usr/share/data", src.path().join("subdir").join("link"))
            .unwrap();

        copy_dotfiles(src.path(), dest.path());

        // Real file should be copied
        assert!(dest.path().join("subdir").join("real.txt").exists());

        // Symlink in subdirectory should be recreated pointing to same target
        let dest_link = dest.path().join("subdir").join("link");
        let meta = std::fs::symlink_metadata(&dest_link).unwrap();
        assert!(meta.file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(&dest_link).unwrap(),
            PathBuf::from("/usr/share/data")
        );
    }

    // ---- copy_dotfiles nested symlink tests (multi-segment passthrough) ----

    #[cfg(unix)]
    #[test]
    fn copy_dotfiles_nested_symlink_entry() {
        // Staging tree has .config/gh as a symlink (created by create_passthrough_symlinks).
        // copy_dotfiles must recreate .config/ dir and gh symlink inside destination.
        let staging = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Create staging/.config/ as a real directory
        std::fs::create_dir_all(staging.path().join(".config")).unwrap();
        // Create staging/.config/gh as a symlink pointing to the host gh dir
        let host_gh = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(host_gh.path(), staging.path().join(".config").join("gh"))
            .unwrap();

        copy_dotfiles(staging.path(), dest.path());

        // dest/.config/ should be a real directory
        let dest_config = dest.path().join(".config");
        assert!(dest_config.is_dir(), "dest/.config should be a directory");

        // dest/.config/gh should be a symlink pointing to host_gh
        let dest_gh = dest_config.join("gh");
        let meta = std::fs::symlink_metadata(&dest_gh).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "dest/.config/gh should be a symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_dotfiles_nested_regular_file() {
        // staging/.foo/bar/baz.txt should be copied to dest/.foo/bar/baz.txt
        let staging = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(staging.path().join(".foo").join("bar")).unwrap();
        std::fs::write(
            staging.path().join(".foo").join("bar").join("baz.txt"),
            "hello",
        )
        .unwrap();

        copy_dotfiles(staging.path(), dest.path());

        let dest_baz = dest.path().join(".foo").join("bar").join("baz.txt");
        assert!(dest_baz.exists(), "nested regular file should be copied");
        assert_eq!(std::fs::read_to_string(&dest_baz).unwrap(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn copy_dotfiles_flat_regression() {
        // Verify that top-level entries still work after any changes.
        let staging = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        std::fs::write(staging.path().join(".gitconfig"), "[user]\n  name = Test").unwrap();
        std::os::unix::fs::symlink("/usr/local", staging.path().join(".local-link")).unwrap();

        copy_dotfiles(staging.path(), dest.path());

        assert!(dest.path().join(".gitconfig").exists());
        let meta = std::fs::symlink_metadata(dest.path().join(".local-link")).unwrap();
        assert!(meta.file_type().is_symlink());
    }

    #[test]
    fn copy_dotfiles_depth_limit_skips_overly_deep_dirs() {
        // Build a directory tree that is exactly MAX_COPY_DEPTH + 1 levels deep.
        // The deepest directory should be skipped rather than causing a stack overflow.
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Build depth = MAX_COPY_DEPTH + 1 directories: src/d0/d1/.../d{MAX+1}
        let mut current = src.path().to_path_buf();
        let too_deep = super::MAX_COPY_DEPTH + 1;
        for i in 0..=too_deep {
            current = current.join(format!("d{}", i));
        }
        std::fs::create_dir_all(&current).unwrap();
        // Place a sentinel file at the deepest level.
        std::fs::write(current.join("deep.txt"), "should be skipped").unwrap();

        // Place a file at a shallow (depth=1) level that should be copied.
        let shallow_dir = src.path().join("d0");
        std::fs::write(shallow_dir.join("shallow.txt"), "should be copied").unwrap();

        copy_dotfiles(src.path(), dest.path());

        // The shallow file should be present.
        assert!(
            dest.path().join("d0").join("shallow.txt").exists(),
            "shallow file must be copied"
        );
        // The overly-deep sentinel must not appear anywhere under dest.
        let deep_in_dest = {
            let mut p = dest.path().to_path_buf();
            for i in 0..=too_deep {
                p = p.join(format!("d{}", i));
            }
            p.join("deep.txt")
        };
        assert!(
            !deep_in_dest.exists(),
            "file beyond MAX_COPY_DEPTH must be skipped"
        );
    }
}
