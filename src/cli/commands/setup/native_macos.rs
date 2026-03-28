//! Native sandbox setup and uninstall for macOS
//!
//! Manages the dedicated system user (`_mino_agent`), privileged helper binary,
//! sudoers entry, and pf anchor configuration.

use super::{
    check_installed_helper_version, find_available_system_uid, run_visible_sudo, StepResult,
};
use crate::cli::args::SetupArgs;
use crate::error::{MinoError, MinoResult};
use crate::ui::{self, UiContext};
use std::process::Stdio;
use tokio::process::Command;

pub(super) async fn setup_native_macos(ctx: &UiContext, args: &SetupArgs) -> MinoResult<()> {
    ui::section(ctx, "Native Sandbox Setup (macOS)");

    let sandbox_user = crate::sandbox::config::DEFAULT_SANDBOX_USER;

    // Step 1: Create system user
    let user_result = setup_sandbox_user(ctx, args, sandbox_user).await;

    // Step 2: Install helper binary
    let helper_result = if user_result.is_ok() {
        install_helper_binary(ctx, args).await
    } else {
        ui::step_blocked(ctx, "Helper Binary", "System User");
        StepResult::Blocked
    };

    // Step 3: Configure sudoers
    let sudoers_result = if helper_result.is_ok() {
        configure_sudoers(ctx, args).await
    } else {
        ui::step_blocked(ctx, "Sudoers", "Helper Binary");
        StepResult::Blocked
    };

    // Step 4: Configure pf anchor
    let pf_result = if sudoers_result.is_ok() {
        configure_pf_anchor(ctx, args, sandbox_user).await
    } else {
        ui::step_blocked(ctx, "pf Anchor", "Sudoers");
        StepResult::Blocked
    };

    // Summary
    let issues = count_setup_issues(&[user_result, helper_result, sudoers_result, pf_result]);

    if issues > 0 {
        ui::outro_warn(ctx, "Native sandbox setup incomplete. See issues above.");
    } else {
        ui::outro_success(
            ctx,
            "Native sandbox ready! Use: mino run --runtime native -- <command>",
        );
    }

    Ok(())
}

/// Remove all native sandbox artifacts on macOS.
///
/// Steps (all require sudo):
/// 1. Kill any running `_mino_agent` processes
/// 2. Flush pf anchor rules
/// 3. Remove pf anchor file
/// 4. Remove sudoers entry
/// 5. Remove helper binary
/// 6. Delete `_mino_agent` system user
pub(super) async fn uninstall_native_macos(ctx: &UiContext) -> MinoResult<()> {
    ui::section(ctx, "Removing native sandbox components...");

    let sandbox_user = crate::sandbox::config::DEFAULT_SANDBOX_USER;

    // 1. Kill any running processes owned by _mino_agent
    let kill_output = Command::new("sudo")
        .args(["pkill", "-u", sandbox_user])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    match kill_output {
        Ok(status) if status.success() => {
            ui::step_ok(ctx, "Killed running _mino_agent processes");
        }
        _ => {
            // pkill exits non-zero if no processes found -- that's fine
            ui::step_ok(ctx, "No running _mino_agent processes");
        }
    }

    // 2. Flush pf anchor rules
    let _ = Command::new("sudo")
        .args(["pfctl", "-a", "mino", "-F", "rules"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    ui::step_ok(ctx, "Flushed pf anchor rules");

    // 3-5. Remove artifacts via shared helper
    remove_if_exists(ctx, "/etc/pf.anchors/mino", "pf anchor file").await;
    remove_if_exists(ctx, SUDOERS_PATH, "sudoers entry").await;
    remove_if_exists(ctx, HELPER_BINARY_PATH, "helper binary").await;

    // 6. Delete _mino_agent system user
    let user_exists = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", sandbox_user)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if user_exists {
        if run_visible_sudo(
            "dscl",
            &[".", "-delete", &format!("/Users/{}", sandbox_user)],
        )
        .await
        {
            ui::step_ok_detail(ctx, "Deleted system user", sandbox_user);
        } else {
            ui::step_warn(
                ctx,
                &format!("Failed to delete system user '{}'", sandbox_user),
            );
        }
    } else {
        ui::step_ok(ctx, "System user already removed");
    }

    ui::outro_success(ctx, "Native sandbox uninstalled.");
    Ok(())
}

// =============================================================================
// Setup steps
// =============================================================================

async fn setup_sandbox_user(ctx: &UiContext, args: &SetupArgs, username: &str) -> StepResult {
    if let Err(e) = crate::sandbox::config::validate_sandbox_user(username) {
        ui::step_error(ctx, &e.to_string());
        return StepResult::Failed;
    }

    // Check if user exists via dscl
    let exists = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", username)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if exists {
        ui::step_ok_detail(ctx, "Sandbox user exists", username);
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error_detail(ctx, "Sandbox user not found", username);
        return StepResult::Failed;
    }

    ui::remark(ctx, &format!("Creating system user '{}'...", username));

    // Find an available UID in the system range (400-499)
    let uid = match find_available_system_uid().await {
        Some(uid) => uid,
        None => {
            ui::step_error(ctx, "Failed to find available UID in range 400-499");
            return StepResult::Failed;
        }
    };

    // Create the user record
    let user_path = format!("/Users/{}", username);
    let uid_str = uid.to_string();

    let steps: &[(&[&str], &str)] = &[
        (&[".", "-create", &user_path], "create user record"),
        (
            &[".", "-create", &user_path, "UserShell", "/usr/bin/false"],
            "set shell",
        ),
        (
            &[".", "-create", &user_path, "RealName", "Mino Sandbox Agent"],
            "set real name",
        ),
        (
            &[".", "-create", &user_path, "UniqueID", &uid_str],
            "set UID",
        ),
        (
            &[".", "-create", &user_path, "PrimaryGroupID", "20"],
            "set group",
        ),
        (
            &[".", "-create", &user_path, "NFSHomeDirectory", "/var/empty"],
            "set home",
        ),
        (
            &[".", "-create", &user_path, "IsHidden", "1"],
            "hide from login screen",
        ),
    ];

    for (dscl_args, description) in steps {
        if !run_visible_sudo("dscl", dscl_args).await {
            ui::step_error_detail(ctx, "Failed to create sandbox user", description);
            return StepResult::Failed;
        }
    }

    ui::step_ok_detail(
        ctx,
        "Sandbox user created",
        &format!("{} (UID {})", username, uid),
    );
    StepResult::Installed
}

async fn install_helper_binary(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let mino_version = env!("CARGO_PKG_VERSION");

    // Check if helper exists and version matches
    let current_version = check_installed_helper_version().await;

    if let Some(version) = current_version {
        if version == mino_version {
            ui::step_ok_detail(ctx, "Helper binary", &format!("v{}", version));
            return StepResult::AlreadyOk;
        }
        ui::remark(
            ctx,
            &format!(
                "Helper version mismatch (v{} vs v{}), upgrading...",
                version, mino_version
            ),
        );
    }

    if args.check {
        ui::step_error(ctx, "Helper binary not installed or outdated");
        return StepResult::Failed;
    }

    // Get path to current mino binary, the helper is built alongside it
    let helper_src = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("mino-sandbox-helper")));

    match helper_src {
        Some(src) if src.exists() => {
            let src_str = src.to_string_lossy();
            let install_success =
                run_visible_sudo("cp", &[&src_str, "/usr/local/bin/mino-sandbox-helper"]).await;

            if install_success {
                // Ensure correct ownership and permissions
                let _ =
                    run_visible_sudo("chmod", &["755", "/usr/local/bin/mino-sandbox-helper"]).await;
                let _ = run_visible_sudo(
                    "chown",
                    &["root:wheel", "/usr/local/bin/mino-sandbox-helper"],
                )
                .await;

                ui::step_ok_detail(
                    ctx,
                    "Helper binary installed",
                    &format!("v{}", mino_version),
                );
                StepResult::Installed
            } else {
                ui::step_error(ctx, "Failed to install helper binary");
                StepResult::Failed
            }
        }
        _ => {
            ui::step_error_detail(
                ctx,
                "Helper binary not found next to mino executable",
                "Build with: cargo build --release",
            );
            StepResult::Failed
        }
    }
}

async fn configure_sudoers(ctx: &UiContext, args: &SetupArgs) -> StepResult {
    let sudoers_file = SUDOERS_PATH;

    if std::path::Path::new(sudoers_file).exists() {
        ui::step_ok(ctx, "Sudoers configured");
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_error(ctx, "Sudoers not configured");
        return StepResult::Failed;
    }

    // Get the current user's username
    let username = std::env::var("USER")
        .unwrap_or_else(|_| std::env::var("LOGNAME").unwrap_or_else(|_| "unknown".to_string()));

    let sudoers_content = match generate_sudoers_content(&username) {
        Ok(content) => content,
        Err(e) => {
            ui::step_error(ctx, &e.to_string());
            return StepResult::Failed;
        }
    };

    // Write to a temp file, then copy via sudo (avoiding sudo tee complexity)
    let tmp_file = std::env::temp_dir().join("mino-sudoers");
    if std::fs::write(&tmp_file, &sudoers_content).is_err() {
        ui::step_error(ctx, "Failed to write temporary sudoers file");
        return StepResult::Failed;
    }

    let tmp_str = tmp_file.to_string_lossy();
    let success = run_visible_sudo("cp", &[&tmp_str, sudoers_file]).await;
    let _ = std::fs::remove_file(&tmp_file);

    if success {
        // sudoers files must be mode 0440
        let _ = run_visible_sudo("chmod", &["0440", sudoers_file]).await;
        // Validate the sudoers file
        let valid = Command::new("sudo")
            .args(["visudo", "-c", "-f", sudoers_file])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if valid {
            ui::step_ok_detail(ctx, "Sudoers configured", &username);
            StepResult::Installed
        } else {
            // Invalid sudoers file -- remove it to avoid locking out sudo
            let _ = run_visible_sudo("rm", &[sudoers_file]).await;
            ui::step_error(ctx, "Sudoers validation failed — file removed");
            StepResult::Failed
        }
    } else {
        ui::step_error(ctx, "Failed to install sudoers file");
        StepResult::Failed
    }
}

async fn configure_pf_anchor(ctx: &UiContext, args: &SetupArgs, sandbox_user: &str) -> StepResult {
    // Check if anchor exists in pf.conf
    let pf_check = Command::new("sudo")
        .args(["pfctl", "-s", "Anchors"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    if let Ok(output) = pf_check {
        let anchors = String::from_utf8_lossy(&output.stdout);
        if anchors.lines().any(|l| l.trim() == "mino") {
            ui::step_ok(ctx, "pf anchor configured");
            return StepResult::AlreadyOk;
        }
    }

    if args.check {
        ui::step_error(ctx, "pf anchor not configured");
        return StepResult::Failed;
    }

    // Generate and write anchor rules
    let anchor_rules = match crate::sandbox::macos::generate_pf_rules(sandbox_user, "default", None)
    {
        Ok(rules) => rules,
        Err(e) => {
            ui::step_error(ctx, &format!("Failed to generate pf rules: {}", e));
            return StepResult::Failed;
        }
    };
    let anchor_file = "/etc/pf.anchors/mino";

    let tmp_file = std::env::temp_dir().join("mino-pf-anchor");
    if std::fs::write(&tmp_file, &anchor_rules).is_err() {
        ui::step_error(ctx, "Failed to write temporary anchor file");
        return StepResult::Failed;
    }

    let tmp_str = tmp_file.to_string_lossy();
    let copy_success = run_visible_sudo("cp", &[&tmp_str, anchor_file]).await;
    let _ = std::fs::remove_file(&tmp_file);

    if !copy_success {
        ui::step_error(ctx, "Failed to install pf anchor file");
        return StepResult::Failed;
    }

    // Load the anchor
    let load_success = Command::new("sudo")
        .args(["pfctl", "-a", "mino", "-f", anchor_file])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if load_success {
        ui::step_ok(ctx, "pf anchor configured and loaded");
        StepResult::Installed
    } else {
        // Anchor file is written but not loaded -- might need pf to be enabled
        ui::step_warn_hint(
            ctx,
            "pf anchor file installed but loading failed",
            "Ensure pf is enabled: sudo pfctl -e",
        );
        StepResult::Installed
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Remove a file if it exists, reporting the outcome via UI.
async fn remove_if_exists(ctx: &UiContext, path: &str, description: &str) {
    if std::path::Path::new(path).exists() {
        if run_visible_sudo("rm", &[path]).await {
            ui::step_ok(ctx, &format!("Removed {}", description));
        } else {
            ui::step_warn(ctx, &format!("Failed to remove {}", description));
        }
    } else {
        ui::step_ok(ctx, &format!("{} already removed", description));
    }
}

// =============================================================================
// Pure functions (testable without root or system state)
// =============================================================================

/// The path where the sudoers drop-in is installed.
const SUDOERS_PATH: &str = "/etc/sudoers.d/mino";

/// The helper binary path referenced in sudoers rules.
const HELPER_BINARY_PATH: &str = "/usr/local/bin/mino-sandbox-helper";

/// Generate sudoers file content that grants a user passwordless sudo access
/// to the mino-sandbox-helper binary.
///
/// Validates the username to prevent injection into the sudoers file. Only
/// alphanumeric characters, underscores, and hyphens are allowed (max 32 chars),
/// matching macOS system username constraints.
///
/// Returns `Err` if the username is empty, too long, or contains disallowed chars.
pub(super) fn generate_sudoers_content(username: &str) -> MinoResult<String> {
    validate_sudoers_username(username)?;
    Ok(format!(
        "{} ALL=(root) NOPASSWD: {}\n",
        username, HELPER_BINARY_PATH
    ))
}

/// Validate that a username is safe for inclusion in a sudoers file.
///
/// Rejects empty, oversized, or special-character usernames that could
/// inject rules or break sudoers syntax.
fn validate_sudoers_username(username: &str) -> MinoResult<()> {
    if username.is_empty() {
        return Err(MinoError::User(
            "Invalid username '' for sudoers (must be 1-32 alphanumeric/underscore/hyphen chars)"
                .to_string(),
        ));
    }
    if username.len() > 32 {
        return Err(MinoError::User(format!(
            "Invalid username '{}' for sudoers (must be 1-32 alphanumeric/underscore/hyphen chars)",
            username
        )));
    }
    if !username
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(MinoError::User(format!(
            "Invalid username '{}' for sudoers (must be 1-32 alphanumeric/underscore/hyphen chars)",
            username
        )));
    }
    Ok(())
}

/// Compute the step gating chain: given a sequence of results, count how many
/// are user-actionable issues (used for the setup summary).
///
/// Blocked steps are NOT counted because they are cascading consequences of an
/// upstream failure and should not inflate the issue total.
pub(super) fn count_setup_issues(results: &[StepResult]) -> usize {
    results.iter().filter(|r| r.is_issue()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sudoers content generation ----

    #[test]
    fn sudoers_content_for_standard_username() {
        let content = generate_sudoers_content("dean").unwrap();
        assert_eq!(
            content,
            "dean ALL=(root) NOPASSWD: /usr/local/bin/mino-sandbox-helper\n"
        );
    }

    #[test]
    fn sudoers_content_for_underscore_username() {
        let content = generate_sudoers_content("_mino_admin").unwrap();
        assert!(content.starts_with("_mino_admin ALL=(root) NOPASSWD:"));
        assert!(content.contains(HELPER_BINARY_PATH));
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn sudoers_content_for_hyphenated_username() {
        let content = generate_sudoers_content("my-user").unwrap();
        assert!(content.starts_with("my-user ALL=(root) NOPASSWD:"));
    }

    #[test]
    fn sudoers_content_for_numeric_username() {
        let content = generate_sudoers_content("user42").unwrap();
        assert!(content.starts_with("user42 ALL=(root) NOPASSWD:"));
    }

    #[test]
    fn sudoers_content_single_char_username() {
        let content = generate_sudoers_content("x").unwrap();
        assert!(content.starts_with("x ALL=(root) NOPASSWD:"));
    }

    #[test]
    fn sudoers_content_max_length_username() {
        let name = "a".repeat(32);
        let content = generate_sudoers_content(&name).unwrap();
        assert!(content.contains(&name));
    }

    #[test]
    fn sudoers_content_ends_with_newline() {
        // sudoers files must end with a newline
        let content = generate_sudoers_content("user").unwrap();
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn sudoers_content_is_single_line() {
        let content = generate_sudoers_content("user").unwrap();
        assert_eq!(content.lines().count(), 1);
    }

    // ---- sudoers username validation (injection prevention) ----

    #[test]
    fn sudoers_rejects_empty_username() {
        let err = generate_sudoers_content("").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_username_over_32_chars() {
        let long = "a".repeat(33);
        let err = generate_sudoers_content(&long).unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_spaces() {
        // Spaces in a sudoers username would break the rule syntax
        let err = generate_sudoers_content("bad user").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_newlines() {
        // Newlines could inject additional sudoers directives
        let err = generate_sudoers_content("user\nALL=(ALL) NOPASSWD: ALL").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_tab_injection() {
        let err = generate_sudoers_content("user\tALL").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_semicolons() {
        let err = generate_sudoers_content("user;evil").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_slashes() {
        let err = generate_sudoers_content("../../etc").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_hash_comment_injection() {
        // '#' could comment out the rest of the rule and leave a partial entry
        let err = generate_sudoers_content("user#comment").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_null_byte() {
        let err = generate_sudoers_content("user\0evil").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_comma() {
        // Commas separate user entries in sudoers
        let err = generate_sudoers_content("user,root").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_equals() {
        let err = generate_sudoers_content("user=root").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    #[test]
    fn sudoers_rejects_parentheses() {
        let err = generate_sudoers_content("user(root)").unwrap_err();
        assert!(err.to_string().contains("Invalid username"));
    }

    // ---- step gating logic ----

    #[test]
    fn count_issues_all_ok() {
        let results = [StepResult::AlreadyOk, StepResult::Installed];
        assert_eq!(count_setup_issues(&results), 0);
    }

    #[test]
    fn count_issues_one_failed() {
        let results = [StepResult::AlreadyOk, StepResult::Failed, StepResult::Blocked];
        // Only Failed counts, Blocked is a cascading consequence
        assert_eq!(count_setup_issues(&results), 1);
    }

    #[test]
    fn count_issues_skipped_counts() {
        let results = [StepResult::Skipped, StepResult::Blocked];
        assert_eq!(count_setup_issues(&results), 1);
    }

    #[test]
    fn count_issues_blocked_not_counted() {
        // Blocked steps are NOT user-actionable issues
        let results = [StepResult::Blocked, StepResult::Blocked, StepResult::Blocked];
        assert_eq!(count_setup_issues(&results), 0);
    }

    #[test]
    fn count_issues_full_cascade() {
        // Simulates: step 1 fails, steps 2-4 blocked
        let results = [
            StepResult::Failed,
            StepResult::Blocked,
            StepResult::Blocked,
            StepResult::Blocked,
        ];
        // Only the root failure counts
        assert_eq!(count_setup_issues(&results), 1);
    }

    #[test]
    fn count_issues_multiple_failures() {
        let results = [
            StepResult::Failed,
            StepResult::Failed,
            StepResult::Skipped,
        ];
        assert_eq!(count_setup_issues(&results), 3);
    }

    #[test]
    fn count_issues_empty() {
        assert_eq!(count_setup_issues(&[]), 0);
    }

    // ---- step gating chains ----

    #[test]
    fn step_gating_success_propagates() {
        // When step 1 succeeds, step 2 should proceed (not be blocked)
        let step1 = StepResult::AlreadyOk;
        let step2 = if step1.is_ok() {
            StepResult::Installed
        } else {
            StepResult::Blocked
        };
        assert_eq!(step2, StepResult::Installed);
    }

    #[test]
    fn step_gating_failure_blocks_downstream() {
        // When step 1 fails, step 2 should be blocked
        let step1 = StepResult::Failed;
        let step2 = if step1.is_ok() {
            StepResult::Installed
        } else {
            StepResult::Blocked
        };
        assert_eq!(step2, StepResult::Blocked);
    }

    #[test]
    fn step_gating_skipped_blocks_downstream() {
        // When step 1 is skipped, step 2 should be blocked
        let step1 = StepResult::Skipped;
        let step2 = if step1.is_ok() {
            StepResult::Installed
        } else {
            StepResult::Blocked
        };
        assert_eq!(step2, StepResult::Blocked);
    }

    #[test]
    fn step_gating_four_step_chain_first_failure() {
        // Simulates the macOS setup flow: user -> helper -> sudoers -> pf
        let step1 = StepResult::Failed;
        let step2 = if step1.is_ok() { StepResult::Installed } else { StepResult::Blocked };
        let step3 = if step2.is_ok() { StepResult::Installed } else { StepResult::Blocked };
        let step4 = if step3.is_ok() { StepResult::Installed } else { StepResult::Blocked };

        assert_eq!(step1, StepResult::Failed);
        assert_eq!(step2, StepResult::Blocked);
        assert_eq!(step3, StepResult::Blocked);
        assert_eq!(step4, StepResult::Blocked);

        let issues = count_setup_issues(&[step1, step2, step3, step4]);
        assert_eq!(issues, 1); // Only root failure counts
    }

    #[test]
    fn step_gating_four_step_chain_all_success() {
        let step1 = StepResult::AlreadyOk;
        let step2 = if step1.is_ok() { StepResult::Installed } else { StepResult::Blocked };
        let step3 = if step2.is_ok() { StepResult::AlreadyOk } else { StepResult::Blocked };
        let step4 = if step3.is_ok() { StepResult::Installed } else { StepResult::Blocked };

        assert!(step4.is_ok());
        let issues = count_setup_issues(&[step1, step2, step3, step4]);
        assert_eq!(issues, 0);
    }

    #[test]
    fn step_gating_four_step_chain_mid_failure() {
        // Step 2 fails, steps 3-4 should be blocked
        let step1 = StepResult::AlreadyOk;
        let step2 = if step1.is_ok() { StepResult::Failed } else { StepResult::Blocked };
        let step3 = if step2.is_ok() { StepResult::Installed } else { StepResult::Blocked };
        let step4 = if step3.is_ok() { StepResult::Installed } else { StepResult::Blocked };

        assert_eq!(step2, StepResult::Failed);
        assert_eq!(step3, StepResult::Blocked);
        assert_eq!(step4, StepResult::Blocked);

        let issues = count_setup_issues(&[step1, step2, step3, step4]);
        assert_eq!(issues, 1);
    }

    // ---- constants ----

    #[test]
    fn sudoers_path_is_standard_drop_in_location() {
        assert_eq!(SUDOERS_PATH, "/etc/sudoers.d/mino");
    }

    #[test]
    fn helper_binary_path_matches_expected() {
        assert_eq!(HELPER_BINARY_PATH, "/usr/local/bin/mino-sandbox-helper");
    }
}
