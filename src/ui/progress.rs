//! Progress indicators with CI fallback

use super::context::UiContext;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

/// A task spinner with CI fallback
pub struct TaskSpinner {
    spinner: Option<cliclack::ProgressBar>,
    message: String,
    interactive: bool,
}

impl TaskSpinner {
    /// Create a new spinner (shows immediately in interactive mode)
    pub fn new(ctx: &UiContext) -> Self {
        Self {
            spinner: None,
            message: String::new(),
            interactive: ctx.use_fancy_output(),
        }
    }

    /// Start the spinner with a message
    pub fn start(&mut self, message: &str) {
        self.message = message.to_string();

        if self.interactive {
            let spinner = cliclack::spinner();
            spinner.start(message);
            self.spinner = Some(spinner);
        } else {
            // Plain output for CI
            println!("{} {}", style("...").dim(), message);
        }
    }

    /// Update the spinner message
    pub fn message(&mut self, message: &str) {
        self.message = message.to_string();

        if let Some(ref spinner) = self.spinner {
            spinner.start(message);
        }
        // No output in plain mode for message updates
    }

    /// Stop with success message
    pub fn stop(&mut self, message: &str) {
        if let Some(spinner) = self.spinner.take() {
            spinner.stop(message);
        } else if self.interactive {
            // Fallback if spinner wasn't started
            println!("{} {}", style("✓").green(), message);
        } else {
            println!("{} {}", style("[OK]").green(), message);
        }
    }

    /// Stop with error message
    pub fn stop_error(&mut self, message: &str) {
        if let Some(spinner) = self.spinner.take() {
            spinner.error(message);
        } else if self.interactive {
            println!("{} {}", style("✗").red(), message);
        } else {
            println!("{} {}", style("[FAIL]").red(), message);
        }
    }

    /// Stop with warning message
    pub fn stop_warn(&mut self, message: &str) {
        if let Some(spinner) = self.spinner.take() {
            spinner.stop(message);
        } else if self.interactive {
            println!("{} {}", style("!").yellow(), message);
        } else {
            println!("{} {}", style("[WARN]").yellow(), message);
        }
    }

    /// Clear the spinner without any message
    pub fn clear(&mut self) {
        if let Some(spinner) = self.spinner.take() {
            spinner.clear();
        }
    }
}

/// Progress bar for container image builds.
///
/// Parses Podman `STEP N/M: <instruction>` lines and displays
/// an indicatif progress bar in interactive mode, or plain text in CI.
pub struct BuildProgress {
    bar: Option<ProgressBar>,
}

impl BuildProgress {
    /// Create a new build progress indicator.
    ///
    /// Shows an indicatif bar in interactive mode, plain text in CI.
    pub fn new(ctx: &UiContext, label: &str) -> Self {
        let bar = if ctx.use_fancy_output() {
            let bar = ProgressBar::new(0);
            bar.set_style(
                ProgressStyle::default_bar()
                    .template("  Building {prefix}: {bar:30.cyan/dim} {pos}/{len} {msg:.dim}")
                    .unwrap()
                    .progress_chars("━╸─"),
            );
            bar.set_prefix(label.to_string());
            Some(bar)
        } else {
            println!("Building {}...", label);
            None
        };
        Self { bar }
    }

    /// Process a build output line. Parses `STEP N/M:` and updates the bar.
    pub fn on_line(&self, line: String) {
        if let Some((n, total, instruction)) = parse_step_line(&line) {
            if let Some(ref bar) = self.bar {
                bar.set_length(total);
                bar.set_position(n);
                bar.set_message(instruction.to_string());
            } else {
                println!("  STEP {}/{}: {}", n, total, instruction);
            }
        }
    }

    /// Finish and clear the progress bar.
    pub fn finish(&self) {
        if let Some(ref bar) = self.bar {
            bar.finish_and_clear();
        }
    }
}

/// Parse a Podman build step line like `STEP N/M: INSTRUCTION args...`
fn parse_step_line(line: &str) -> Option<(u64, u64, &str)> {
    let rest = line.strip_prefix("STEP ")?;
    let slash = rest.find('/')?;
    let colon = rest.find(':')?;
    if colon <= slash {
        return None;
    }
    let n: u64 = rest[..slash].parse().ok()?;
    let total: u64 = rest[slash + 1..colon].parse().ok()?;
    let instruction = rest[colon + 1..].trim();
    Some((n, total, instruction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_non_interactive() {
        let ctx = UiContext::non_interactive();
        let mut spinner = TaskSpinner::new(&ctx);
        spinner.start("Testing...");
        spinner.stop("Done");
        // Should not panic
    }

    #[test]
    fn parse_step_line_valid() {
        let (n, m, instr) = parse_step_line("STEP 3/13: RUN chmod +x /tmp/install.sh").unwrap();
        assert_eq!(n, 3);
        assert_eq!(m, 13);
        assert_eq!(instr, "RUN chmod +x /tmp/install.sh");
    }

    #[test]
    fn parse_step_line_from_instruction() {
        let (n, m, instr) = parse_step_line("STEP 1/8: FROM ghcr.io/dean0x/mino-base:latest").unwrap();
        assert_eq!(n, 1);
        assert_eq!(m, 8);
        assert_eq!(instr, "FROM ghcr.io/dean0x/mino-base:latest");
    }

    #[test]
    fn parse_step_line_not_a_step() {
        assert!(parse_step_line("---> abc123def").is_none());
        assert!(parse_step_line("Removing intermediate container").is_none());
        assert!(parse_step_line("").is_none());
    }

    #[test]
    fn build_progress_non_interactive() {
        let ctx = UiContext::non_interactive();
        let progress = BuildProgress::new(&ctx, "typescript");
        progress.on_line("STEP 1/5: FROM base:latest".to_string());
        progress.on_line("---> abc123".to_string());
        progress.finish();
        // Should not panic
    }
}
