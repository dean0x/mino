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
unsafe fn set_rlimit(resource: RlimitResource, value: u64, name: &str) {
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
