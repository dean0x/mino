//! UI module for consistent, modern CLI experience
//!
//! Uses `cliclack` (Rust port of @clack/prompts) for interactive prompts
//! with automatic fallback to plain output in CI/non-interactive environments.
//!
//! # Example
//!
//! ```rust,ignore
//! use mino::ui::{self, UiContext, TaskSpinner};
//!
//! let ctx = UiContext::detect().with_auto_yes(args.yes);
//!
//! ui::intro(&ctx, "Mino Setup");
//!
//! let mut spinner = TaskSpinner::new(&ctx);
//! spinner.start("Checking prerequisites...");
//! // ... do work ...
//! spinner.stop("All prerequisites found");
//!
//! ui::step_ok(&ctx, "Homebrew installed");
//! ui::step_warn_hint(&ctx, "OrbStack not running", "Run: orb start");
//!
//! let yes = ui::confirm(&ctx, "Install OrbStack?", false).await?;
//!
//! ui::outro_success(&ctx, "Setup complete!");
//! ```

mod context;
mod output;
mod progress;
mod prompts;
mod theme;

pub use context::UiContext;
pub use output::{
    intro, key_value, key_value_status, note, outro_error, outro_success, outro_warn, remark,
    section, step_blocked, step_error, step_error_detail, step_info, step_ok, step_ok_detail,
    step_warn, step_warn_hint,
};
pub use progress::TaskSpinner;
pub use prompts::{confirm, confirm_inline, select};
pub use theme::{init_theme, MinoTheme};
