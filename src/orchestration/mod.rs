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
pub use podman::{ContainerConfig, ContainerInfo};
pub use runtime::{ContainerRuntime, VolumeInfo};

use tokio::io::{AsyncBufReadExt, BufReader};

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
