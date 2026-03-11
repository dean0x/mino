//! Orchestration module for container runtimes
//!
//! Provides platform-agnostic container management:
//! - macOS: OrbStack VM + Podman
//! - Linux: Native rootless Podman

mod factory;
mod native_podman;
pub mod orbstack;
mod orbstack_runtime;
pub mod podman;
mod runtime;

pub use factory::{create_runtime, create_runtime_with_vm, Platform};
pub use orbstack::OrbStack;
pub use podman::ContainerConfig;
pub use runtime::{ContainerRuntime, VolumeInfo};

use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::MinoResult;

/// Max number of output lines to include in build error messages.
const BUILD_ERROR_TAIL_LINES: usize = 50;

/// Extract the useful tail of build output for error diagnostics.
///
/// Combines stdout and stderr, then returns the last `BUILD_ERROR_TAIL_LINES`
/// lines so error messages are actionable without being overwhelming.
pub(crate) fn build_error_output(stdout: &str, stderr: &str) -> String {
    let lines: Vec<&str> = stdout.lines().chain(stderr.lines()).collect();
    let total = lines.len();
    let tail: Vec<&str> = if total > BUILD_ERROR_TAIL_LINES {
        lines[total - BUILD_ERROR_TAIL_LINES..].to_vec()
    } else {
        lines
    };
    tail.join("\n")
}

/// Stream stdout+stderr from a child process, calling `on_output` for each line.
///
/// Returns all collected output lines for error reporting. This is a standalone
/// async function (not behind `async_trait`) to avoid lifetime issues with the
/// `dyn Fn` callback.
pub(crate) async fn stream_child_output(
    child: &mut tokio::process::Child,
    on_output: &(dyn Fn(String) + Send + Sync),
) -> Vec<String> {
    let stderr = child.stderr.take().expect("stderr piped");
    let stdout = child.stdout.take().expect("stdout piped");

    let mut stderr_reader = BufReader::new(stderr).lines();
    let mut stdout_reader = BufReader::new(stdout).lines();

    let mut all_output = Vec::new();
    let mut stderr_done = false;
    let mut stdout_done = false;

    while !stderr_done || !stdout_done {
        tokio::select! {
            line = stderr_reader.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => {
                        on_output(line.clone());
                        all_output.push(line);
                    }
                    _ => stderr_done = true,
                }
            }
            line = stdout_reader.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => {
                        on_output(line.clone());
                        all_output.push(line);
                    }
                    _ => stdout_done = true,
                }
            }
        }
    }

    all_output
}

/// Parse `du -sb` output to extract the byte size.
///
/// `du -sb` prints `<bytes>\t<path>` -- this extracts and parses the leading
/// number, returning `None` if the output cannot be parsed.
pub(crate) fn parse_du_bytes(output: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(output);
    text.split_whitespace()
        .next()
        .and_then(|s| s.parse::<u64>().ok())
}

/// Collect volume disk usage results from a batch of parallel futures.
///
/// Each future should resolve to `Ok(Some((name, size)))` on success or
/// `Ok(None)` when the size could not be determined. Errors propagate.
pub(crate) fn collect_disk_usage(
    results: Vec<MinoResult<Option<(String, u64)>>>,
) -> MinoResult<HashMap<String, u64>> {
    let mut sizes = HashMap::new();
    for result in results {
        if let Some((name, size)) = result? {
            sizes.insert(name, size);
        }
    }
    Ok(sizes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MinoError;

    // -- parse_du_bytes --

    #[test]
    fn parse_du_bytes_valid() {
        let output = b"12345\t/var/lib/containers/storage/volumes/vol/_data\n";
        assert_eq!(parse_du_bytes(output), Some(12345));
    }

    #[test]
    fn parse_du_bytes_large_value() {
        let output = b"1073741824\t/some/path\n";
        assert_eq!(parse_du_bytes(output), Some(1_073_741_824));
    }

    #[test]
    fn parse_du_bytes_empty() {
        assert_eq!(parse_du_bytes(b""), None);
    }

    #[test]
    fn parse_du_bytes_non_numeric() {
        assert_eq!(parse_du_bytes(b"abc\t/path\n"), None);
    }

    #[test]
    fn parse_du_bytes_whitespace_only() {
        assert_eq!(parse_du_bytes(b"   \t  \n"), None);
    }

    // -- collect_disk_usage --

    #[test]
    fn collect_disk_usage_happy_path() {
        let results = vec![
            Ok(Some(("vol-a".to_string(), 100))),
            Ok(Some(("vol-b".to_string(), 200))),
        ];
        let sizes = collect_disk_usage(results).unwrap();
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes["vol-a"], 100);
        assert_eq!(sizes["vol-b"], 200);
    }

    #[test]
    fn collect_disk_usage_skips_none() {
        let results = vec![
            Ok(Some(("vol-a".to_string(), 100))),
            Ok(None),
            Ok(Some(("vol-c".to_string(), 300))),
        ];
        let sizes = collect_disk_usage(results).unwrap();
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes["vol-a"], 100);
        assert_eq!(sizes["vol-c"], 300);
    }

    #[test]
    fn collect_disk_usage_empty() {
        let results: Vec<MinoResult<Option<(String, u64)>>> = vec![];
        let sizes = collect_disk_usage(results).unwrap();
        assert!(sizes.is_empty());
    }

    #[test]
    fn collect_disk_usage_propagates_error() {
        let results = vec![
            Ok(Some(("vol-a".to_string(), 100))),
            Err(MinoError::Internal("test error".to_string())),
        ];
        let err = collect_disk_usage(results).unwrap_err();
        assert!(err.to_string().contains("test error"));
    }
}
