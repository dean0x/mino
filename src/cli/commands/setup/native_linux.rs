//! Native sandbox setup for Linux (user namespaces + unshare)

use super::StepResult;
use crate::cli::args::SetupArgs;
use crate::config::ConfigManager;
use crate::error::MinoResult;
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

pub(super) async fn setup_native_linux(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
    ui::section(ctx, "Native Sandbox Setup (Linux)");

    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let config_path = ConfigManager::default_config_path();

    // Step 1: Verify user namespace support
    let userns_result = super::check_user_namespaces(ctx, args).await;

    // Step 2: Check unshare is available
    let unshare_result = check_unshare(ctx).await;

    // Step 3: Offer toolchain passthrough
    let toolchain_result =
        super::helpers::configure_toolchain_passthrough(ctx, args, &home, &config_path).await;

    // Step 4: Offer sensitive-but-useful passthrough (skipped in non-interactive)
    let sensitive_result =
        super::helpers::configure_sensitive_passthrough(ctx, args, &home, &config_path).await;

    // Step 5: Offer .claude auto-copy
    let claude_result =
        super::helpers::configure_claude_auto_copy(ctx, args, &home, &config_path).await;

    // Summary
    let issues = [
        userns_result,
        unshare_result,
        toolchain_result,
        sensitive_result,
        claude_result,
    ]
    .iter()
    .filter(|r| r.is_issue())
    .count();

    if issues > 0 {
        ui::outro_warn(
            ctx,
            "Native sandbox prerequisites not met. See issues above.",
        );
    } else {
        ui::outro_success(
            ctx,
            "Native sandbox ready! Use: mino run --runtime native -- <command>",
        );
    }

    Ok(())
}

async fn check_unshare(ctx: &UiContext) -> StepResult {
    let output = Command::new("which")
        .arg("unshare")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout);
            ui::step_ok_detail(ctx, "unshare available", path.trim());
            StepResult::AlreadyOk
        }
        _ => {
            ui::step_error_detail(
                ctx,
                "unshare not found",
                "Install util-linux for user namespace support",
            );
            StepResult::Failed
        }
    }
}
