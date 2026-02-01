//! Output functions for consistent CLI formatting

use super::context::UiContext;
use console::{style, Style};

/// Display intro banner
pub fn intro(ctx: &UiContext, title: &str) {
    if ctx.use_fancy_output() {
        cliclack::intro(style(title).cyan().bold()).ok();
    } else {
        println!("{}", style(title).cyan().bold());
        println!();
    }
}

/// Display success outro
pub fn outro_success(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::outro(style(message).green().bold()).ok();
    } else {
        println!();
        println!("{} {}", style("[OK]").green(), message);
    }
}

/// Display error outro
pub fn outro_error(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::outro(style(message).red().bold()).ok();
    } else {
        println!();
        println!("{} {}", style("[ERROR]").red(), message);
    }
}

/// Display warning outro
pub fn outro_warn(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::outro(style(message).yellow().bold()).ok();
    } else {
        println!();
        println!("{} {}", style("[WARN]").yellow(), message);
    }
}

/// Display a note/info box
pub fn note(ctx: &UiContext, title: &str, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::note(title, message).ok();
    } else {
        println!("{}: {}", style(title).bold(), message);
    }
}

/// Display a section header
pub fn section(ctx: &UiContext, title: &str) {
    if ctx.use_fancy_output() {
        println!();
        cliclack::log::info(style(title).bold()).ok();
    } else {
        println!();
        println!("{}", style(title).bold());
    }
}

/// Display a success step
pub fn step_ok(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::success(message).ok();
    } else {
        println!("  {} {}", style("[OK]").green(), message);
    }
}

/// Display a success step with detail
pub fn step_ok_detail(ctx: &UiContext, message: &str, detail: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::success(format!("{} ({})", message, style(detail).dim())).ok();
    } else {
        println!("  {} {} ({})", style("[OK]").green(), message, detail);
    }
}

/// Display a warning step
pub fn step_warn(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::warning(message).ok();
    } else {
        println!("  {} {}", style("[WARN]").yellow(), message);
    }
}

/// Display a warning step with hint
pub fn step_warn_hint(ctx: &UiContext, message: &str, hint: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::warning(format!("{} - {}", message, style(hint).dim())).ok();
    } else {
        println!("  {} {} - {}", style("[WARN]").yellow(), message, hint);
    }
}

/// Display an error step
pub fn step_error(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::error(message).ok();
    } else {
        println!("  {} {}", style("[FAIL]").red(), message);
    }
}

/// Display an error step with detail
pub fn step_error_detail(ctx: &UiContext, message: &str, detail: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::error(format!("{}: {}", message, style(detail).red())).ok();
    } else {
        println!("  {} {}: {}", style("[FAIL]").red(), message, detail);
    }
}

/// Display an info step
pub fn step_info(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::info(message).ok();
    } else {
        println!("  {} {}", style("[INFO]").cyan(), message);
    }
}

/// Display a blocked/skipped step
pub fn step_blocked(ctx: &UiContext, name: &str, dependency: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::info(format!(
            "{} - {}",
            style(name).dim(),
            style(format!("blocked (requires {})", dependency)).dim()
        )).ok();
    } else {
        println!("  {} {} (requires {})", style("[-]").dim(), name, dependency);
    }
}

/// Display a remark/hint
pub fn remark(ctx: &UiContext, message: &str) {
    if ctx.use_fancy_output() {
        cliclack::log::remark(message).ok();
    } else {
        println!("  {}", style(message).dim());
    }
}

/// Print styled key-value pair
pub fn key_value(ctx: &UiContext, key: &str, value: &str) {
    if ctx.use_fancy_output() {
        println!("  {}: {}", style(key).dim(), value);
    } else {
        println!("  {}: {}", key, value);
    }
}

/// Print styled key-value with status color
pub fn key_value_status(ctx: &UiContext, key: &str, value: &str, ok: bool) {
    let value_style = if ok {
        Style::new().green()
    } else {
        Style::new().yellow()
    };

    if ctx.use_fancy_output() {
        println!("  {}: {}", style(key).dim(), value_style.apply_to(value));
    } else {
        let prefix = if ok { "[OK]" } else { "[WARN]" };
        println!("  {} {}: {}", prefix, key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_non_interactive() {
        let ctx = UiContext::non_interactive();
        // These should not panic
        intro(&ctx, "Test");
        outro_success(&ctx, "Done");
        step_ok(&ctx, "Step completed");
        step_warn(&ctx, "Warning");
        step_error(&ctx, "Error");
    }
}
