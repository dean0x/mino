use std::sync::atomic::{AtomicI32, Ordering};

/// Global child PID for signal forwarding
///
/// Stored as an atomic to avoid `static mut` unsoundness.
/// Written once in parent_process() before signal handlers fire,
/// read only in the signal handler. Single-threaded binary.
#[cfg(unix)]
pub(crate) static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// Set up signal forwarding to child process
///
/// # Safety
/// Installs signal handlers via `sigaction(2)` with `SA_RESTART`.
/// Must be called only once, from the parent process after fork().
#[cfg(unix)]
pub(crate) unsafe fn setup_signal_forwarding(child_pid: i32) {
    CHILD_PID.store(child_pid, Ordering::SeqCst);

    let mut action: libc::sigaction = std::mem::zeroed();
    action.sa_sigaction = forward_signal as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    libc::sigemptyset(&mut action.sa_mask);

    libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
}

/// C-compatible signal handler that forwards signals to the child
///
/// # Safety
/// This is a signal handler. It only calls async-signal-safe functions
/// (libc::kill). Reads CHILD_PID atomically; the value was stored before
/// handler installation.
#[cfg(unix)]
extern "C" fn forward_signal(sig: libc::c_int) {
    let pid = CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}
