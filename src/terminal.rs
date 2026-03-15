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

        // Only save if stdin is a real terminal
        if unsafe { libc::isatty(fd) } != 1 {
            return None;
        }

        let mut termios = std::mem::MaybeUninit::uninit();
        let result = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
        if result == 0 {
            Some(Self {
                fd,
                original: unsafe { termios.assume_init() },
            })
        } else {
            None
        }
    }

    /// Explicitly restore terminal state.
    pub(crate) fn restore(&self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}

#[cfg(unix)]
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_returns_none_in_ci() {
        // In test/CI environments, stdin is typically not a terminal
        // This test verifies save() handles non-terminal gracefully
        let guard = TerminalGuard::save();
        // Whether it returns Some or None depends on the environment,
        // but it must not panic
        drop(guard);
    }
}
