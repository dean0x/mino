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
                    .template("  {spinner:.cyan} Building {prefix}  {bar:20.cyan/dim} {pos}/{len} {msg:.dim}  {elapsed:.dim}")
                    .unwrap()
                    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
                    .progress_chars("━╸─"),
            );
            bar.set_prefix(label.to_string());
            bar.enable_steady_tick(std::time::Duration::from_millis(120));
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
        } else if let Some(ref bar) = self.bar {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !is_build_noise(trimmed) {
                let display = if trimmed.len() > 60 {
                    format!("{}...", &trimmed[..57])
                } else {
                    trimmed.to_string()
                };
                bar.set_message(display);
            }
        }
    }

    /// Finish and clear the progress bar.
    pub fn finish(&self) {
        if let Some(ref bar) = self.bar {
            bar.disable_steady_tick();
            bar.finish_and_clear();
        }
    }
}

/// Filter out Podman internal build lines that aren't useful to display.
fn is_build_noise(line: &str) -> bool {
    line.starts_with("--->")
        || line.starts_with("-->")
        || line.starts_with("Removing intermediate")
        || line.starts_with("COMMIT")
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
        progress.on_line("downloading rustup-init".to_string());
        progress.finish();
        // Should not panic
    }

    #[test]
    fn is_build_noise_filters_podman_internals() {
        assert!(is_build_noise("---> abc123def"));
        assert!(is_build_noise("--> Using cache abc123"));
        assert!(is_build_noise("Removing intermediate container abc123"));
        assert!(is_build_noise("COMMIT mino-composed-abc123"));
        assert!(!is_build_noise("downloading rustup-init"));
        assert!(!is_build_noise("Compiling mino v1.0.0"));
        assert!(!is_build_noise(""));
    }
}
