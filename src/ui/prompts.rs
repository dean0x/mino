//! Interactive prompts with CI/non-interactive fallback

use super::context::UiContext;
use crate::error::MinoResult;
use std::io::{self, Write};

/// Prompt for confirmation, returns default if non-interactive or auto-yes
pub async fn confirm(ctx: &UiContext, message: &str, default: bool) -> MinoResult<bool> {
    // Auto-yes mode bypasses prompts
    if ctx.auto_yes() {
        println!("  {} (auto-approved)", message);
        return Ok(true);
    }

    // Non-interactive mode returns default
    if !ctx.is_interactive() {
        return Ok(default);
    }

    // Run blocking cliclack prompt in spawn_blocking
    let message = message.to_string();
    let result = tokio::task::spawn_blocking(move || {
        cliclack::confirm(&message)
            .initial_value(default)
            .interact()
    })
    .await
    .map_err(|e| crate::error::MinoError::User(format!("Prompt task failed: {}", e)))?;

    result.map_err(|e| crate::error::MinoError::User(format!("Prompt failed: {}", e)))
}

/// Prompt for selection from a list of options
/// Returns the selected value or the first option if non-interactive
pub async fn select<T: Clone + Send + Eq + 'static>(
    ctx: &UiContext,
    message: &str,
    options: &[(T, &str, &str)], // (value, label, hint)
) -> MinoResult<T> {
    // Non-interactive mode returns first option
    if !ctx.is_interactive() || ctx.auto_yes() {
        return Ok(options[0].0.clone());
    }

    // Build cliclack select
    let message = message.to_string();
    let items: Vec<(T, String, String)> = options
        .iter()
        .map(|(v, l, h)| (v.clone(), l.to_string(), h.to_string()))
        .collect();

    let result: Result<Result<T, std::io::Error>, _> = tokio::task::spawn_blocking(move || {
        let mut select = cliclack::select(&message);
        for (value, label, hint) in items {
            select = select.item(value, label, hint);
        }
        select.interact()
    })
    .await;

    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(crate::error::MinoError::User(format!(
            "Select failed: {}",
            e
        ))),
        Err(e) => Err(crate::error::MinoError::User(format!(
            "Select task failed: {}",
            e
        ))),
    }
}

/// Simple inline confirmation for non-fancy mode (used by setup)
pub fn confirm_inline(prompt: &str, auto_yes: bool) -> bool {
    if auto_yes {
        println!("  {} (auto-approved)", prompt);
        return true;
    }

    print!("  {} [y/N] ", prompt);
    if io::stdout().flush().is_err() {
        return false;
    }

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    input.trim().eq_ignore_ascii_case("y")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn confirm_auto_yes() {
        let ctx = UiContext::non_interactive().with_auto_yes(true);
        let result = confirm(&ctx, "Test?", false).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn confirm_non_interactive_default() {
        let ctx = UiContext::non_interactive();
        let result = confirm(&ctx, "Test?", true).await.unwrap();
        assert!(result);

        let result = confirm(&ctx, "Test?", false).await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn select_non_interactive_first() {
        let ctx = UiContext::non_interactive();
        let options = vec![
            ("a".to_string(), "Option A", "First"),
            ("b".to_string(), "Option B", "Second"),
        ];
        let result = select(&ctx, "Choose:", &options).await.unwrap();
        assert_eq!(result, "a");
    }
}
