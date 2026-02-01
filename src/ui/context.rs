//! UI context for detecting interactive vs CI environments

use std::io::IsTerminal;

/// UI context that determines output behavior
#[derive(Debug, Clone)]
pub struct UiContext {
    /// Whether running in an interactive terminal
    interactive: bool,
    /// Whether --yes flag was passed (auto-approve prompts)
    auto_yes: bool,
}

impl UiContext {
    /// Detect the current environment
    pub fn detect() -> Self {
        let interactive = Self::detect_interactive();
        Self {
            interactive,
            auto_yes: false,
        }
    }

    /// Create a non-interactive context (for testing or explicit CI mode)
    pub fn non_interactive() -> Self {
        Self {
            interactive: false,
            auto_yes: false,
        }
    }

    /// Set auto-yes mode (bypass prompts with defaults)
    pub fn with_auto_yes(mut self, yes: bool) -> Self {
        self.auto_yes = yes;
        self
    }

    /// Check if we're in an interactive terminal
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    /// Check if prompts should be auto-approved
    pub fn auto_yes(&self) -> bool {
        self.auto_yes
    }

    /// Check if we should use fancy output (spinners, colors)
    pub fn use_fancy_output(&self) -> bool {
        self.interactive
    }

    /// Detect if running in an interactive environment
    fn detect_interactive() -> bool {
        // Not interactive if stdout is not a TTY
        if !std::io::stdout().is_terminal() {
            return false;
        }

        // Not interactive if stdin is not a TTY
        if !std::io::stdin().is_terminal() {
            return false;
        }

        // Check for CI environment variables
        if std::env::var("CI").is_ok() {
            return false;
        }

        // Common CI environment indicators
        let ci_vars = [
            "GITHUB_ACTIONS",
            "GITLAB_CI",
            "CIRCLECI",
            "TRAVIS",
            "JENKINS_URL",
            "BUILDKITE",
            "TEAMCITY_VERSION",
            "TF_BUILD",
        ];

        for var in ci_vars {
            if std::env::var(var).is_ok() {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interactive_context() {
        let ctx = UiContext::non_interactive();
        assert!(!ctx.is_interactive());
        assert!(!ctx.auto_yes());
    }

    #[test]
    fn with_auto_yes() {
        let ctx = UiContext::non_interactive().with_auto_yes(true);
        assert!(ctx.auto_yes());
    }
}
