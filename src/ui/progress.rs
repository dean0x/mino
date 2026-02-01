//! Progress indicators with CI fallback

use super::context::UiContext;
use console::style;

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
}
