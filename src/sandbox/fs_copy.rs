//! Generic filesystem copy utilities for the native sandbox.
//!
//! Provides an iterative (non-recursive) directory copy that avoids per-level
//! heap allocation and handles symlink skipping without following them into the
//! source tree.

use crate::error::{MinoError, MinoResult};
use std::collections::VecDeque;
use std::path::PathBuf;

/// Recursively copy a directory tree from `src` to `dst`.
///
/// Uses an iterative BFS worklist instead of async recursion to avoid
/// one `Box::pin` allocation per directory level.
///
/// **Symlink skipping**: entries detected as symlinks via
/// [`tokio::fs::symlink_metadata`] are silently skipped. This prevents
/// the sandbox staging directory from receiving host-relative symlinks
/// that would dangle inside the container.
///
/// # Errors
/// Returns an error if any directory creation, directory read, or file copy
/// fails. On error the destination tree may be partially written; callers are
/// responsible for cleanup.
pub async fn copy_dir_recursive(src: PathBuf, dst: PathBuf) -> MinoResult<()> {
    let mut queue: VecDeque<(PathBuf, PathBuf)> = VecDeque::from([(src, dst)]);

    while let Some((src_dir, dst_dir)) = queue.pop_front() {
        tokio::fs::create_dir_all(&dst_dir)
            .await
            .map_err(|e| MinoError::io("creating copy dir", e))?;

        let mut entries = tokio::fs::read_dir(&src_dir)
            .await
            .map_err(|e| MinoError::io("reading dir", e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| MinoError::io("reading dir entry", e))?
        {
            let src_path = entry.path();
            let dst_path = dst_dir.join(entry.file_name());

            // Use symlink_metadata so we inspect the entry itself, not its target.
            let meta = tokio::fs::symlink_metadata(&src_path)
                .await
                .map_err(|e| MinoError::io("stat", e))?;

            if meta.file_type().is_symlink() {
                // Skip symlinks — copying their target could pull in host paths
                // that become dangling inside the sandbox.
                continue;
            }

            if meta.is_dir() {
                queue.push_back((src_path, dst_path));
            } else if meta.is_file() {
                tokio::fs::copy(&src_path, &dst_path)
                    .await
                    .map_err(|e| {
                        MinoError::io(
                            format!("copying {}", src_path.display()),
                            e,
                        )
                    })?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copies_files_and_subdirs() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path().to_path_buf();
        let dst = dst_guard.path().join("dest");

        tokio::fs::write(src.join("file.txt"), b"hello")
            .await
            .unwrap();
        tokio::fs::create_dir_all(src.join("subdir"))
            .await
            .unwrap();
        tokio::fs::write(src.join("subdir").join("nested.txt"), b"world")
            .await
            .unwrap();

        copy_dir_recursive(src.clone(), dst.clone())
            .await
            .unwrap();

        assert!(dst.join("file.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(dst.join("file.txt"))
                .await
                .unwrap(),
            "hello"
        );
        assert!(dst.join("subdir").join("nested.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(dst.join("subdir").join("nested.txt"))
                .await
                .unwrap(),
            "world"
        );
    }

    #[tokio::test]
    async fn skips_symlinks() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path().to_path_buf();
        let dst = dst_guard.path().join("dest");

        // Regular file that should be copied
        tokio::fs::write(src.join("real.txt"), b"real content")
            .await
            .unwrap();

        // Symlink that must be skipped (T-003)
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("/etc/passwd", src.join("sneaky.link")).unwrap();
        }

        copy_dir_recursive(src.clone(), dst.clone())
            .await
            .unwrap();

        assert!(dst.join("real.txt").exists(), "regular file should be copied");
        assert!(
            !dst.join("sneaky.link").exists(),
            "symlink must not be copied into destination"
        );
    }

    #[tokio::test]
    async fn handles_empty_source_dir() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path().to_path_buf();
        let dst = dst_guard.path().join("dest");

        copy_dir_recursive(src, dst.clone()).await.unwrap();

        assert!(dst.exists());
    }

    #[tokio::test]
    async fn errors_on_missing_src() {
        let dst_guard = tempfile::tempdir().unwrap();
        let src = dst_guard.path().join("does_not_exist");
        let dst = dst_guard.path().join("dest");

        let result = copy_dir_recursive(src, dst).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn copies_deeply_nested_tree() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path().to_path_buf();
        let dst = dst_guard.path().join("dest");

        // Build a/b/c/deep.txt
        let deep = src.join("a").join("b").join("c");
        tokio::fs::create_dir_all(&deep).await.unwrap();
        tokio::fs::write(deep.join("deep.txt"), b"deep").await.unwrap();

        copy_dir_recursive(src, dst.clone()).await.unwrap();

        assert!(dst.join("a").join("b").join("c").join("deep.txt").exists());
    }
}
