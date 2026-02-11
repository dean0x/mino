//! Custom theme for cliclack prompts

use cliclack::ThemeState;
use console::Style;

/// Mino's custom theme with cyan branding
#[derive(Debug, Clone, Default)]
pub struct MinoTheme;

impl cliclack::Theme for MinoTheme {
    fn bar_color(&self, state: &ThemeState) -> Style {
        match state {
            ThemeState::Active => Style::new().cyan(),
            ThemeState::Error(_) => Style::new().red(),
            ThemeState::Cancel => Style::new().dim(),
            ThemeState::Submit => Style::new().cyan().dim(),
        }
    }

    fn state_symbol_color(&self, state: &ThemeState) -> Style {
        match state {
            ThemeState::Active => Style::new().cyan(),
            ThemeState::Error(_) => Style::new().red(),
            ThemeState::Cancel => Style::new().dim(),
            ThemeState::Submit => Style::new().green(),
        }
    }
}

/// Initialize the global theme
pub fn init_theme() {
    cliclack::set_theme(MinoTheme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cliclack::Theme;

    #[test]
    fn theme_colors() {
        let theme = MinoTheme;
        // Just verify we can call the methods
        let _ = theme.bar_color(&ThemeState::Active);
        let _ = theme.state_symbol_color(&ThemeState::Submit);
    }
}
