//! Pure helper functions extracted from setup submodules.
//!
//! These are small, testable functions that replace inline patterns
//! across container_linux, container_macos, and native_macos setup flows.

use super::StepResult;
use crate::cli::args::SetupArgs;
use crate::config::ConfigManager;
use crate::sandbox::dotfiles::{
    detect_claude_copy_candidate, detect_passthrough_candidates, detect_sensitive_candidates,
    CLAUDE_AUTO_COPY_CANDIDATE, SENSITIVE_BUT_USEFUL_CANDIDATES, TOOLCHAIN_PASSTHROUGH_CANDIDATES,
};
use crate::ui::{self, UiContext};
use std::path::Path;

/// Detect toolchain directories on the host and add them to
/// `[sandbox].auto_passthrough_dirs` so shell init files can source them
/// without "no such file or directory" errors.
///
/// Safe directories (not credential stores) are detected from
/// `TOOLCHAIN_PASSTHROUGH_CANDIDATES`. In interactive mode the user
/// can deselect individual entries; in non-interactive / CI mode all
/// detected entries are accepted automatically.
///
/// Entries already present in the config are preserved and deduplicated.
pub(super) async fn configure_toolchain_passthrough(
    ctx: &UiContext,
    args: &SetupArgs,
    home: &Path,
    config_path: &Path,
) -> StepResult {
    let detected = detect_passthrough_candidates(
        home,
        TOOLCHAIN_PASSTHROUGH_CANDIDATES,
        crate::sandbox::config::SENSITIVE_PATHS,
    );

    if detected.is_empty() {
        ui::step_ok(ctx, "Toolchain dirs: none detected");
        return StepResult::AlreadyOk;
    }

    // Read existing config to avoid clobbering manually set entries
    let manager = ConfigManager::with_path(config_path.to_path_buf());
    let existing = match manager.read_sandbox_passthrough_dirs().await {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            ui::step_warn_hint(
                ctx,
                "Could not read existing passthrough dirs",
                &e.to_string(),
            );
            vec![]
        }
    };

    // In check mode just report whether config is already populated
    if args.check {
        if existing.is_empty() {
            ui::step_warn_hint(
                ctx,
                "Toolchain dirs not configured",
                "Run: mino setup --native to auto-detect",
            );
            return StepResult::Failed;
        }
        ui::step_ok_detail(
            ctx,
            "Toolchain passthrough configured",
            &format!("{} dirs", existing.len()),
        );
        return StepResult::AlreadyOk;
    }

    // Determine which detected entries the user wants to add.
    // In non-interactive mode: accept all detected entries automatically.
    let to_add: Vec<String> = if ctx.is_interactive() && !ctx.auto_yes() {
        let options: Vec<(String, &str, &str)> = detected
            .iter()
            .map(|d| (d.clone(), d.as_str(), ""))
            .collect();
        let options_ref: Vec<(String, &str, &str)> = options
            .iter()
            .map(|(v, l, h)| (v.clone(), *l, *h))
            .collect();
        match ui::multiselect(ctx, "Select toolchain dirs to passthrough:", &options_ref, false)
            .await
        {
            Ok(selected) => selected,
            Err(e) => {
                ui::step_warn_hint(ctx, "Toolchain selection failed", &e.to_string());
                return StepResult::Skipped;
            }
        }
    } else {
        detected.clone()
    };

    if to_add.is_empty() {
        ui::step_ok(ctx, "Toolchain passthrough: no dirs selected");
        return StepResult::Skipped;
    }

    // Merge: keep existing + add newly selected (dedup, preserve order)
    let mut merged: Vec<String> = existing;
    let mut newly_added = 0usize;
    for entry in &to_add {
        if !merged.contains(entry) {
            merged.push(entry.clone());
            newly_added += 1;
        }
    }

    if newly_added == 0 {
        // All selected entries were already present
        ui::step_ok_detail(
            ctx,
            "Toolchain passthrough",
            "already configured, no changes",
        );
        return StepResult::AlreadyOk;
    }

    match manager.set_sandbox_passthrough_dirs(&merged).await {
        Ok(()) => {
            ui::step_ok_detail(
                ctx,
                "Toolchain passthrough configured",
                &format!("{} dir(s) added", newly_added),
            );
            StepResult::Installed
        }
        Err(e) => {
            ui::step_error_detail(ctx, "Failed to write config", &e.to_string());
            StepResult::Failed
        }
    }
}

/// Detect sensitive-but-useful directories (credential stores that many AI
/// workflows require) and offer to add them to `auto_passthrough_dirs` AND
/// `allow_sensitive_paths` so `SandboxConfig::validate()` permits them.
///
/// Always skipped in non-interactive / CI mode (security default: don't add
/// credential dirs without explicit user consent).
pub(super) async fn configure_sensitive_passthrough(
    ctx: &UiContext,
    args: &SetupArgs,
    home: &Path,
    config_path: &Path,
) -> StepResult {
    let detected = detect_sensitive_candidates(home, SENSITIVE_BUT_USEFUL_CANDIDATES);

    if detected.is_empty() {
        ui::step_ok(ctx, "Sensitive dirs: none detected");
        return StepResult::AlreadyOk;
    }

    if args.check {
        // Check mode: non-blocking, sensitive dirs are optional
        ui::step_ok(ctx, "Sensitive dirs: not checked (optional)");
        return StepResult::AlreadyOk;
    }

    // Security policy: skip in non-interactive mode — never auto-add credential dirs
    if !ctx.is_interactive() || ctx.auto_yes() {
        ui::step_ok(ctx, "Sensitive dirs: skipped (non-interactive)");
        return StepResult::AlreadyOk;
    }

    let manager = ConfigManager::with_path(config_path.to_path_buf());
    let existing_passthrough = match manager.read_sandbox_passthrough_dirs().await {
        Ok(v) => v.unwrap_or_default(),
        Err(_) => vec![],
    };
    let existing_sensitive = match manager.read_sandbox_allow_sensitive_paths().await {
        Ok(v) => v.unwrap_or_default(),
        Err(_) => vec![],
    };

    // Filter out already-configured entries
    let not_yet_added: Vec<String> = detected
        .iter()
        .filter(|d| !existing_passthrough.contains(d))
        .cloned()
        .collect();

    if not_yet_added.is_empty() {
        ui::step_ok(ctx, "Sensitive passthrough: already configured");
        return StepResult::AlreadyOk;
    }

    ui::note(
        ctx,
        "Warning: credential directories detected",
        "The following directories contain credentials (GitHub token, Docker auth, etc.). Adding them gives the sandbox read access to those credentials.",
    );

    let options: Vec<(String, &str, &str)> = not_yet_added
        .iter()
        .map(|d| (d.clone(), d.as_str(), "contains credentials"))
        .collect();
    let options_ref: Vec<(String, &str, &str)> = options
        .iter()
        .map(|(v, l, h)| (v.clone(), *l, *h))
        .collect();

    let selected = match ui::multiselect(
        ctx,
        "Select sensitive dirs to allow (optional):",
        &options_ref,
        false,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            ui::step_warn_hint(ctx, "Sensitive dir selection failed", &e.to_string());
            return StepResult::Skipped;
        }
    };

    if selected.is_empty() {
        ui::step_ok(ctx, "Sensitive passthrough: none selected");
        return StepResult::AlreadyOk;
    }

    // Write both keys atomically in a single file operation
    let mut new_passthrough = existing_passthrough;
    for s in &selected {
        if !new_passthrough.contains(s) {
            new_passthrough.push(s.clone());
        }
    }
    let mut new_sensitive = existing_sensitive;
    for s in &selected {
        if !new_sensitive.contains(s) {
            new_sensitive.push(s.clone());
        }
    }

    match manager
        .write_toml_keys(&[
            ("auto_passthrough_dirs", &new_passthrough),
            ("allow_sensitive_paths", &new_sensitive),
        ])
        .await
    {
        Ok(()) => {
            ui::step_ok_detail(
                ctx,
                "Sensitive passthrough configured",
                &format!("{} dir(s) added", selected.len()),
            );
            StepResult::Installed
        }
        Err(e) => {
            ui::step_error_detail(ctx, "Failed to write config", &e.to_string());
            StepResult::Failed
        }
    }
}

/// Detect whether `~/.claude` exists and offer to add it to
/// `[sandbox].auto_copy_dirs` so the agent's skills, commands, and
/// project memory are available inside the sandbox.
///
/// Only the allowlisted subset of `~/.claude` is copied (see
/// `copy_claude_config_dir`), so large session/debug directories are excluded.
pub(super) async fn configure_claude_auto_copy(
    ctx: &UiContext,
    args: &SetupArgs,
    home: &Path,
    config_path: &Path,
) -> StepResult {
    let candidate = detect_claude_copy_candidate(home);

    if candidate.is_none() {
        ui::step_ok(ctx, ".claude dir: not found");
        return StepResult::AlreadyOk;
    }

    let manager = ConfigManager::with_path(config_path.to_path_buf());
    let existing = match manager.read_sandbox_copy_dirs().await {
        Ok(v) => v.unwrap_or_default(),
        Err(_) => vec![],
    };

    if existing.contains(&CLAUDE_AUTO_COPY_CANDIDATE.to_string()) {
        ui::step_ok(ctx, ".claude auto-copy: already configured");
        return StepResult::AlreadyOk;
    }

    if args.check {
        ui::step_warn_hint(
            ctx,
            ".claude not in auto_copy_dirs",
            "Run: mino setup --native to configure",
        );
        return StepResult::Failed;
    }

    // In non-interactive mode: skip (credentials may be inside .claude/settings.json)
    if !ctx.is_interactive() || ctx.auto_yes() {
        ui::step_ok(ctx, ".claude auto-copy: skipped (non-interactive)");
        return StepResult::AlreadyOk;
    }

    let confirmed = match ui::confirm(
        ctx,
        "Copy ~/.claude (skills, commands, memory) into sandbox?",
        true,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            ui::step_warn_hint(ctx, ".claude confirm failed", &e.to_string());
            return StepResult::Skipped;
        }
    };

    if !confirmed {
        ui::step_ok(ctx, ".claude auto-copy: declined");
        return StepResult::AlreadyOk;
    }

    let mut new_copy_dirs = existing;
    new_copy_dirs.push(CLAUDE_AUTO_COPY_CANDIDATE.to_string());

    match manager.set_sandbox_copy_dirs(&new_copy_dirs).await {
        Ok(()) => {
            ui::step_ok(ctx, ".claude auto-copy configured");
            StepResult::Installed
        }
        Err(e) => {
            ui::step_error_detail(ctx, "Failed to write config", &e.to_string());
            StepResult::Failed
        }
    }
}

/// Extract the first line from command output, returning "unknown" if empty.
///
/// Replaces inline `output.lines().next().unwrap_or("unknown")` patterns
/// found in container_linux and container_macos version parsing.
pub(super) fn parse_first_line(output: &str) -> &str {
    output.lines().next().unwrap_or("unknown")
}

/// Check whether a distro name is apt-based (requires `apt-get update` before install).
///
/// Replaces inline `distro == "ubuntu" || distro == "debian"` checks
/// in container_macos podman install/upgrade flows.
pub(super) fn is_apt_based_distro(distro: &str) -> bool {
    matches!(distro, "ubuntu" | "debian")
}

/// Generate a subuid/subgid entry for rootless Podman.
///
/// Replaces inline `format!("{}:100000:65536", username)` in container_macos.
pub(super) fn generate_subid_entry(username: &str) -> String {
    format!("{}:100000:65536", username)
}

/// Check whether Podman is running in rootless mode from `podman info` output.
///
/// Replaces inline `stdout.trim() == "true"` in container_linux.
pub(super) fn is_rootless_mode(output: &str) -> bool {
    output.trim() == "true"
}

/// Check whether the "mino" pf anchor is registered in `pfctl -s Anchors` output.
///
/// Replaces inline `anchors.lines().any(|l| l.trim() == "mino")` in native_macos.
pub(super) fn anchor_registered(anchors_output: &str) -> bool {
    anchors_output.lines().any(|l| l.trim() == "mino")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_first_line ----

    #[test]
    fn parse_first_line_single_line() {
        assert_eq!(
            parse_first_line("podman version 4.9.3"),
            "podman version 4.9.3"
        );
    }

    #[test]
    fn parse_first_line_multi_line() {
        assert_eq!(
            parse_first_line("podman version 4.9.3\nsome other info"),
            "podman version 4.9.3"
        );
    }

    #[test]
    fn parse_first_line_empty() {
        assert_eq!(parse_first_line(""), "unknown");
    }

    #[test]
    fn parse_first_line_only_newline() {
        assert_eq!(parse_first_line("\n"), "");
    }

    #[test]
    fn parse_first_line_whitespace_preserved() {
        assert_eq!(parse_first_line("  version 1.0  "), "  version 1.0  ");
    }

    #[test]
    fn parse_first_line_crlf() {
        assert_eq!(parse_first_line("line1\r\nline2"), "line1");
    }

    // ---- is_apt_based_distro ----

    #[test]
    fn apt_based_ubuntu() {
        assert!(is_apt_based_distro("ubuntu"));
    }

    #[test]
    fn apt_based_debian() {
        assert!(is_apt_based_distro("debian"));
    }

    #[test]
    fn apt_based_fedora_is_not() {
        assert!(!is_apt_based_distro("fedora"));
    }

    #[test]
    fn apt_based_arch_is_not() {
        assert!(!is_apt_based_distro("arch"));
    }

    #[test]
    fn apt_based_empty_is_not() {
        assert!(!is_apt_based_distro(""));
    }

    #[test]
    fn apt_based_case_sensitive() {
        assert!(!is_apt_based_distro("Ubuntu"));
    }

    // ---- generate_subid_entry ----

    #[test]
    fn subid_entry_standard() {
        assert_eq!(generate_subid_entry("dean"), "dean:100000:65536");
    }

    #[test]
    fn subid_entry_underscore_user() {
        assert_eq!(
            generate_subid_entry("_mino_agent"),
            "_mino_agent:100000:65536"
        );
    }

    #[test]
    fn subid_entry_empty_username() {
        assert_eq!(generate_subid_entry(""), ":100000:65536");
    }

    // ---- is_rootless_mode ----

    #[test]
    fn rootless_true() {
        assert!(is_rootless_mode("true"));
    }

    #[test]
    fn rootless_true_with_trailing_newline() {
        assert!(is_rootless_mode("true\n"));
    }

    #[test]
    fn rootless_true_with_whitespace() {
        assert!(is_rootless_mode("  true  "));
    }

    #[test]
    fn rootless_false() {
        assert!(!is_rootless_mode("false"));
    }

    #[test]
    fn rootless_empty() {
        assert!(!is_rootless_mode(""));
    }

    #[test]
    fn rootless_random_text() {
        assert!(!is_rootless_mode("maybe"));
    }

    // ---- anchor_registered ----

    #[test]
    fn anchor_present_alone() {
        assert!(anchor_registered("mino"));
    }

    #[test]
    fn anchor_present_among_others() {
        assert!(anchor_registered("com.apple\nmino\ncustom"));
    }

    #[test]
    fn anchor_present_with_whitespace() {
        assert!(anchor_registered("  mino  "));
    }

    #[test]
    fn anchor_absent() {
        assert!(!anchor_registered("com.apple\ncustom"));
    }

    #[test]
    fn anchor_empty() {
        assert!(!anchor_registered(""));
    }

    #[test]
    fn anchor_substring_not_matched() {
        // "minotaur" contains "mino" as substring but should not match
        assert!(!anchor_registered("minotaur"));
    }

    #[test]
    fn anchor_partial_line_not_matched() {
        assert!(!anchor_registered("com.mino.anchor"));
    }
}
