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
    results.into_iter().filter_map(|r| r.transpose()).collect()
}

/// Extract labels from a Podman volume JSON object.
///
/// Podman represents volume labels as `{"Labels": {"key": "value", ...}}`.
/// Non-string values are silently skipped. Returns an empty map when the
/// `Labels` field is missing, null, or not an object.
pub(crate) fn parse_volume_labels(vol: &serde_json::Value) -> HashMap<String, String> {
    vol["Labels"]
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `podman volume ls --format json` output into a filtered list of `VolumeInfo`.
///
/// Volumes whose names do not start with `prefix` are excluded. Empty or
/// whitespace-only stdout is treated as an empty list (not a parse error).
pub(crate) fn parse_volume_list_json(stdout: &str, prefix: &str) -> MinoResult<Vec<VolumeInfo>> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }

    let volumes: Vec<serde_json::Value> = serde_json::from_str(stdout)?;

    let result = volumes
        .iter()
        .filter_map(|vol| {
            let name = vol["Name"].as_str()?;
            if !name.starts_with(prefix) {
                return None;
            }
            Some(volume_info_from_json(vol, name))
        })
        .collect();

    Ok(result)
}

/// Build a `VolumeInfo` from a Podman volume JSON object using the given name.
fn volume_info_from_json(vol: &serde_json::Value, name: &str) -> VolumeInfo {
    VolumeInfo {
        name: name.to_string(),
        labels: parse_volume_labels(vol),
        mountpoint: vol["Mountpoint"].as_str().map(String::from),
        created_at: vol["CreatedAt"].as_str().map(String::from),
        size_bytes: None,
    }
}

/// Parse `podman volume inspect --format json` output into an optional `VolumeInfo`.
///
/// Podman inspect returns a JSON array even for a single volume. Returns `None`
/// when the array is empty. The `name` parameter is used as the canonical volume
/// name (preserving existing behavior where callers pass the requested name rather
/// than trusting the JSON `Name` field).
pub(crate) fn parse_volume_inspect_json(
    stdout: &str,
    name: &str,
) -> MinoResult<Option<VolumeInfo>> {
    if stdout.trim().is_empty() {
        return Ok(None);
    }

    let volumes: Vec<serde_json::Value> = serde_json::from_str(stdout)?;

    Ok(volumes.first().map(|vol| volume_info_from_json(vol, name)))
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

    // -- parse_volume_labels --

    #[test]
    fn parse_volume_labels_from_valid_object() {
        let vol = serde_json::json!({
            "Labels": {
                "io.mino.cache": "true",
                "io.mino.cache.ecosystem": "npm"
            }
        });
        let labels = parse_volume_labels(&vol);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels["io.mino.cache"], "true");
        assert_eq!(labels["io.mino.cache.ecosystem"], "npm");
    }

    #[test]
    fn parse_volume_labels_null() {
        let vol = serde_json::json!({ "Labels": null });
        let labels = parse_volume_labels(&vol);
        assert!(labels.is_empty());
    }

    #[test]
    fn parse_volume_labels_missing() {
        let vol = serde_json::json!({ "Name": "test" });
        let labels = parse_volume_labels(&vol);
        assert!(labels.is_empty());
    }

    #[test]
    fn parse_volume_labels_non_string_values() {
        let vol = serde_json::json!({
            "Labels": {
                "str_label": "value",
                "int_label": 42,
                "bool_label": true
            }
        });
        let labels = parse_volume_labels(&vol);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels["str_label"], "value");
    }

    #[test]
    fn parse_volume_labels_empty_object() {
        let vol = serde_json::json!({ "Labels": {} });
        let labels = parse_volume_labels(&vol);
        assert!(labels.is_empty());
    }

    // -- parse_volume_list_json --

    #[test]
    fn parse_volume_list_json_single_volume() {
        let json = r#"[{
            "Name": "mino-cache-npm-abc123",
            "Labels": {"io.mino.cache": "true"},
            "Mountpoint": "/var/lib/volumes/test/_data",
            "CreatedAt": "2026-03-10T12:00:00Z"
        }]"#;
        let result = parse_volume_list_json(json, "mino-cache-").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "mino-cache-npm-abc123");
        assert_eq!(result[0].labels["io.mino.cache"], "true");
        assert_eq!(
            result[0].mountpoint.as_deref(),
            Some("/var/lib/volumes/test/_data")
        );
        assert_eq!(
            result[0].created_at.as_deref(),
            Some("2026-03-10T12:00:00Z")
        );
        assert!(result[0].size_bytes.is_none());
    }

    #[test]
    fn parse_volume_list_json_multiple_with_prefix_filter() {
        let json = r#"[
            {"Name": "mino-cache-npm-abc", "Labels": {}},
            {"Name": "other-volume", "Labels": {}},
            {"Name": "mino-cache-cargo-def", "Labels": {}}
        ]"#;
        let result = parse_volume_list_json(json, "mino-cache-").unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "mino-cache-npm-abc");
        assert_eq!(result[1].name, "mino-cache-cargo-def");
    }

    #[test]
    fn parse_volume_list_json_empty_string() {
        let result = parse_volume_list_json("", "mino-cache-").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_volume_list_json_whitespace_only() {
        let result = parse_volume_list_json("   \n  \t  ", "mino-cache-").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_volume_list_json_empty_array() {
        let result = parse_volume_list_json("[]", "mino-cache-").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_volume_list_json_no_prefix_match() {
        let json = r#"[{"Name": "other-volume", "Labels": {}}]"#;
        let result = parse_volume_list_json(json, "mino-cache-").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_volume_list_json_null_labels() {
        let json = r#"[{"Name": "mino-cache-npm-abc", "Labels": null}]"#;
        let result = parse_volume_list_json(json, "mino-cache-").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].labels.is_empty());
    }

    #[test]
    fn parse_volume_list_json_missing_optional_fields() {
        let json = r#"[{"Name": "mino-cache-npm-abc"}]"#;
        let result = parse_volume_list_json(json, "mino-cache-").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].labels.is_empty());
        assert!(result[0].mountpoint.is_none());
        assert!(result[0].created_at.is_none());
    }

    #[test]
    fn parse_volume_list_json_invalid_json() {
        let err = parse_volume_list_json("not json", "mino-cache-").unwrap_err();
        assert!(matches!(err, MinoError::Json(_)));
    }

    // -- parse_volume_inspect_json --

    #[test]
    fn parse_volume_inspect_json_single_volume() {
        let json = r#"[{
            "Name": "mino-cache-npm-abc123",
            "Mountpoint": "/var/lib/volumes/test/_data",
            "CreatedAt": "2026-03-10T12:00:00Z"
        }]"#;
        let result = parse_volume_inspect_json(json, "my-vol").unwrap().unwrap();
        // Uses the passed name, not the JSON Name field
        assert_eq!(result.name, "my-vol");
        assert_eq!(
            result.mountpoint.as_deref(),
            Some("/var/lib/volumes/test/_data")
        );
        assert_eq!(result.created_at.as_deref(), Some("2026-03-10T12:00:00Z"));
    }

    #[test]
    fn parse_volume_inspect_json_empty_string() {
        let result = parse_volume_inspect_json("", "my-vol").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_volume_inspect_json_whitespace_only() {
        let result = parse_volume_inspect_json("   \n  \t  ", "my-vol").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_volume_inspect_json_empty_array() {
        let result = parse_volume_inspect_json("[]", "my-vol").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_volume_inspect_json_with_labels() {
        let json = r#"[{
            "Name": "vol",
            "Labels": {"io.mino.cache.state": "complete", "io.mino.cache.hash": "abc123"}
        }]"#;
        let result = parse_volume_inspect_json(json, "vol").unwrap().unwrap();
        assert_eq!(result.labels.len(), 2);
        assert_eq!(result.labels["io.mino.cache.state"], "complete");
        assert_eq!(result.labels["io.mino.cache.hash"], "abc123");
    }

    #[test]
    fn parse_volume_inspect_json_null_labels() {
        let json = r#"[{"Name": "vol", "Labels": null}]"#;
        let result = parse_volume_inspect_json(json, "vol").unwrap().unwrap();
        assert!(result.labels.is_empty());
    }

    #[test]
    fn parse_volume_inspect_json_missing_optional_fields() {
        let json = r#"[{"Name": "vol"}]"#;
        let result = parse_volume_inspect_json(json, "vol").unwrap().unwrap();
        assert!(result.mountpoint.is_none());
        assert!(result.created_at.is_none());
        assert!(result.size_bytes.is_none());
    }

    #[test]
    fn parse_volume_inspect_json_invalid_json() {
        let err = parse_volume_inspect_json("not json", "vol").unwrap_err();
        assert!(matches!(err, MinoError::Json(_)));
    }

    #[test]
    fn parse_volume_inspect_json_non_string_label_values() {
        let json = r#"[{
            "Name": "vol",
            "Labels": {"valid": "yes", "number": 99, "nested": {"a": 1}}
        }]"#;
        let result = parse_volume_inspect_json(json, "vol").unwrap().unwrap();
        assert_eq!(result.labels.len(), 1);
        assert_eq!(result.labels["valid"], "yes");
    }
}
