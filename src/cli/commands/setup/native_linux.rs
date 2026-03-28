//! Native sandbox setup for Linux (user namespaces + unshare)

use super::StepResult;
use crate::error::MinoResult;
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

use crate::cli::args::SetupArgs;

pub(super) async fn setup_native_linux(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
    ui::section(ctx, "Native Sandbox Setup (Linux)");

    // Step 1: Verify user namespace support
    let userns_result = super::check_user_namespaces(ctx, args).await;

    // Step 2: Check unshare is available
    let unshare_result = check_unshare(ctx).await;

    // Summary
    let issues = [userns_result, unshare_result]
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
