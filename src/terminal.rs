//! Terminal state management
//!
//! Saves and restores terminal settings (termios) to prevent corruption
//! when interactive container sessions modify the terminal.

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

/// RAII guard that saves terminal state on creation and restores it on drop.
///
/// Returns `None` from `save()` when stdin is not a terminal (CI, pipes).
#[cfg(unix)]
pub(crate) struct TerminalGuard {
    fd: i32,
    original: libc::termios,
}

#[cfg(unix)]
impl TerminalGuard {
    /// Save current terminal state. Returns `None` if stdin is not a terminal.
    pub(crate) fn save() -> Option<Self> {
        let fd = std::io::stdin().as_raw_fd();

        // SAFETY: isatty is safe to call with any file descriptor; it simply
        // returns 1 if fd refers to a terminal and 0 otherwise.
        if unsafe { libc::isatty(fd) } != 1 {
            return None;
        }

        let mut termios = std::mem::MaybeUninit::uninit();
        // SAFETY: fd is a valid file descriptor obtained from stdin, and
        // termios.as_mut_ptr() points to properly aligned, writable memory.
        let result = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
        if result == 0 {
            Some(Self {
                fd,
                // SAFETY: tcgetattr returned 0 (success), so termios has been
                // fully initialized with valid terminal attributes.
                original: unsafe { termios.assume_init() },
            })
        } else {
            None
        }
    }

    /// Explicitly restore terminal state.
    pub(crate) fn restore(&self) {
        // SAFETY: self.fd is a valid file descriptor from stdin, and
        // self.original contains attributes previously read by tcgetattr.
        // TCSADRAIN waits for pending output to drain before restoring.
        let ret = unsafe { libc::tcsetattr(self.fd, libc::TCSADRAIN, &self.original) };
        if ret != 0 {
            eprintln!(
                "mino: failed to restore terminal attributes (errno: {})",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(unix)]
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn save_returns_none_when_not_a_tty() {
        let guard = TerminalGuard::save();
        // In CI/test environments stdin is redirected (not a TTY),
        // so save() must return None. On a real terminal it returns Some.
        if unsafe { libc::isatty(std::io::stdin().as_raw_fd()) } != 1 {
            assert!(guard.is_none(), "expected None for non-TTY stdin");
        } else {
            assert!(guard.is_some(), "expected Some for TTY stdin");
        }
    }
}
