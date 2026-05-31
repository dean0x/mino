use mino::sandbox::helper_protocol::ResourceLimitsDto;

/// The platform-specific type for rlimit resource identifiers.
/// Linux uses `__rlimit_resource_t` (u32), macOS uses `c_int` (i32).
#[cfg(target_os = "linux")]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(target_os = "linux"))]
type RlimitResource = libc::c_int;

/// Apply POSIX resource limits via setrlimit
///
/// # Safety
/// Calls libc::setrlimit which is an FFI call. Safe when called with
/// valid rlimit values. Zero values are treated as "no limit" and skipped.
/// Failures are logged to stderr but are non-fatal — the sandbox still
/// runs with default OS limits for the failed resource.
#[cfg(unix)]
pub(crate) unsafe fn apply_resource_limits(limits: &ResourceLimitsDto) {
    #[cfg(target_os = "linux")]
    set_rlimit(libc::RLIMIT_AS, limits.max_memory_bytes, "RLIMIT_AS");
    set_rlimit(
        libc::RLIMIT_NPROC,
        u64::from(limits.max_processes),
        "RLIMIT_NPROC",
    );
    set_rlimit(libc::RLIMIT_CPU, limits.max_cpu_seconds, "RLIMIT_CPU");
    set_rlimit(
        libc::RLIMIT_FSIZE,
        limits.max_file_size_bytes,
        "RLIMIT_FSIZE",
    );
}

/// Set a single resource limit. Zero values are skipped (no limit).
///
/// # Safety
/// Calls libc::setrlimit. Must be called before dropping root privileges.
#[cfg(unix)]
pub(crate) unsafe fn set_rlimit(resource: RlimitResource, value: u64, name: &str) {
    if value == 0 {
        return;
    }
    let rlim = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if libc::setrlimit(resource, &rlim) != 0 {
        eprintln!(
            "[mino-helper] setrlimit {} failed: {}",
            name,
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mino::sandbox::helper_protocol::ResourceLimitsDto;

    /// Verify that apply_resource_limits returns without panic or error when all
    /// values are zero — exercising the zero-skip guard in set_rlimit.
    ///
    /// Zero values are skipped before the setrlimit FFI call, so this test
    /// requires no special OS privileges and does not touch kernel limits.
    #[cfg(unix)]
    #[test]
    fn apply_resource_limits_all_zero_is_noop() {
        let limits = ResourceLimitsDto {
            max_memory_bytes: 0,
            max_processes: 0,
            max_cpu_seconds: 0,
            max_file_size_bytes: 0,
        };
        // Safety: zero values are skipped before the setrlimit FFI call, so no
        // kernel interaction occurs and no root privileges are required.
        unsafe {
            apply_resource_limits(&limits);
        }
    }

    /// Verify the zero-skip guard in set_rlimit directly: calling with value == 0
    /// must not alter the existing kernel limit for RLIMIT_NOFILE.
    ///
    /// RLIMIT_NOFILE (per-process FD limit) is readable on any Unix system without
    /// root, so we can assert the limit is unchanged after the skipped call.
    #[cfg(unix)]
    #[test]
    fn set_rlimit_zero_value_does_not_change_existing_limit() {
        let mut before = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // Safety: getrlimit is always safe to call.
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut before) };

        // Safety: zero value — the guard returns before calling setrlimit.
        unsafe { set_rlimit(libc::RLIMIT_NOFILE, 0, "RLIMIT_NOFILE") };

        let mut after = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // Safety: getrlimit is always safe to call.
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut after) };

        assert_eq!(
            before.rlim_cur, after.rlim_cur,
            "set_rlimit with value=0 must not change the soft limit"
        );
    }
}
