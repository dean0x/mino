//! Safe dotfile handling for native sandbox
//!
//! Copies dotfiles into the sandbox home directory with secret stripping.
//! Known credential-bearing files have their secrets removed before copying.

use crate::error::{MinoError, MinoResult};
use std::path::Path;

/// Dotfiles that are always copied (safe defaults)
pub const DEFAULT_DOTFILES: &[&str] = &[
    ".gitconfig",
    ".config/git/ignore",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".tmux.conf",
];

/// Host directories that were previously auto-mounted read-only.
///
/// These are no longer applied by default. To opt in, add to your config:
/// ```toml
/// [sandbox]
/// auto_passthrough_dirs = [".oh-my-zsh", ".nvm", ".zsh"]
/// ```
#[allow(dead_code)]
pub const AUTO_PASSTHROUGH_DIRS: &[&str] = &[".oh-my-zsh", ".nvm", ".zsh"];

/// Host directories that were previously auto-copied into the sandbox home.
///
/// These are no longer applied by default. To opt in, add to your config:
/// ```toml
/// [sandbox]
/// auto_copy_dirs = [".claude"]
/// ```
///
/// Note: `.claude` contains `settings.json` (may hold API tokens), agent
/// definitions, and memory files. Only enable if you understand the implications.
#[allow(dead_code)]
pub const AUTO_COPY_DIRS: &[&str] = &[".claude"];

/// Known-risky dotfiles that trigger warnings
const RISKY_DOTFILES: &[&str] = &[
    ".npmrc",
    ".pypirc",
    ".docker/config.json",
    ".cargo/credentials.toml",
];

/// Strip credential-related lines from .gitconfig content.
///
/// Removes entire `[credential]` and `[credential "..."]` sections,
/// which typically contain `helper = ...` lines that could leak tokens.
/// All other configuration (name, email, aliases, etc.) is preserved.
pub(crate) fn strip_gitconfig_secrets(content: &str) -> String {
    let mut result = Vec::new();
    let mut in_credential_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect start of a [credential] or [credential "..."] section.
        // Also handles trailing inline comments like `[credential]  # comment`.
        // We strip the comment/whitespace suffix before checking for the closing `]`.
        let before_comment = trimmed
            .split_once('#')
            .or_else(|| trimmed.split_once(';'))
            .map_or(trimmed, |(before, _)| before.trim_end());

        if before_comment.starts_with("[credential") && before_comment.ends_with(']') {
            in_credential_section = true;
            continue;
        }

        // Any new section header ends the credential section
        if trimmed.starts_with('[') && !trimmed.starts_with("[credential") {
            in_credential_section = false;
        }

        if !in_credential_section {
            result.push(line);
        }
    }

    // Join and strip trailing whitespace from the section removal
    let output = result.join("\n");
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// Check if a dotfile path is known-risky (may contain auth tokens)
pub(crate) fn is_risky_dotfile(path: &str) -> bool {
    RISKY_DOTFILES.iter().any(|risky| path.ends_with(risky))
}

/// Prepare dotfile content for sandbox (strip secrets from known files).
///
/// Dispatches to the appropriate secret-stripping function based on the
/// dotfile path. Unknown files are returned as-is.
///
/// Note: `$HOME`/`~` references in shell configs are NOT rewritten here.
/// Instead, symlinks in the sandbox home bridge to host directories, so
/// both read paths (config) and write paths (state) work correctly.
pub(crate) fn prepare_dotfile_content(dotfile_path: &str, content: &str) -> String {
    if dotfile_path.ends_with(".gitconfig") {
        strip_gitconfig_secrets(content)
    } else {
        content.to_string()
    }
}

/// Top-level entries to copy from `~/.claude` (allowlist).
///
/// This allowlist exists because `~/.claude` can contain multi-GB state
/// directories (`sessions/`, `file-history/`, `debug/`) that are never
/// needed inside the sandbox. We copy only the small set of configuration
/// files the agent needs for context.
///
/// **Source of truth**: These paths match the Claude Code layout as of the
/// time this allowlist was written. If Claude Code restructures its home
/// directory, this list must be updated.
const CLAUDE_ALLOW_ENTRIES: &[&str] =
    &["CLAUDE.md", "settings.json", "agents", "commands", "skills"];

/// Entries in `~/.claude` that are intentionally excluded (large or transient).
///
/// These are skipped silently — they are large runtime-generated directories that
/// have no value inside the sandbox. Any entry not in this list AND not in
/// [`CLAUDE_ALLOW_ENTRIES`] triggers a forward-compatibility log message so we
/// notice when Claude Code adds new top-level config files we should copy.
const KNOWN_SKIP_ENTRIES: &[&str] = &[
    "sessions",
    "file-history",
    "debug",
    "telemetry",
    "logs",
    "cache",
    "tmp",
    ".git",
    "projects",
];

/// Copy the user's `~/.claude` directory into the sandbox using an allowlist.
///
/// Only the entries in [`CLAUDE_ALLOW_ENTRIES`] are copied. The current
/// project's memory directory (`projects/<project-key>/`) is also copied
/// so the agent has access to project context.
///
/// Project directory keys follow the Claude Code convention:
/// the absolute path with every `/` replaced by `-`, e.g.
/// `/Users/dean/Sandbox/minotaur` → `-Users-dean-Sandbox-minotaur`.
///
/// # Parameters
/// - `src`: path to `~/.claude` on the host
/// - `dst`: staging destination directory (created if absent)
/// - `project_dir`: absolute path to the current project directory
pub async fn copy_claude_config_dir(src: &Path, dst: &Path, project_dir: &Path) -> MinoResult<()> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(|e| MinoError::io("creating .claude copy dir", e))?;

    // Copy allowlisted top-level entries (files and directories).
    for name in CLAUDE_ALLOW_ENTRIES {
        let src_path = src.join(name);
        let dst_path = dst.join(name);
        let metadata = match tokio::fs::symlink_metadata(&src_path).await {
            Ok(m) => m,
            Err(_) => continue, // Entry doesn't exist — skip silently
        };
        if metadata.is_file() {
            tokio::fs::copy(&src_path, &dst_path)
                .await
                .map_err(|e| MinoError::io(format!("copying .claude/{}", name), e))?;
        } else if metadata.is_dir() {
            super::fs_copy::copy_dir_recursive(src_path, dst_path).await?;
        }
        // Skip symlinks — they would dangle inside the sandbox
    }

    // Copy only the current project's memory directory so the agent has context.
    // Project dirs are keyed by their absolute path with '/' replaced by '-'.
    let project_key = project_dir.to_string_lossy().replace('/', "-");
    let projects_src = src.join("projects").join(&project_key);
    if projects_src.is_dir() {
        let projects_dst = dst.join("projects").join(&project_key);
        super::fs_copy::copy_dir_recursive(projects_src, projects_dst).await?;
    }

    // Forward-compatibility: log entries that are neither allowlisted nor in the
    // known-skip list. This fires when Claude Code adds new top-level config files
    // that should potentially be copied — a signal to update CLAUDE_ALLOW_ENTRIES.
    // Directory-scan failure here is non-fatal; the copy already completed above.
    if let Ok(mut scan) = tokio::fs::read_dir(src).await {
        while let Ok(Some(entry)) = scan.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !CLAUDE_ALLOW_ENTRIES.contains(&name_str.as_ref())
                && !KNOWN_SKIP_ENTRIES.contains(&name_str.as_ref())
            {
                tracing::info!(
                    entry = %name_str,
                    "unknown entry in ~/.claude — not in allowlist or known-skip list; \
                     consider updating CLAUDE_ALLOW_ENTRIES if this is a config file"
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_gitconfig_removes_credential_section() {
        let input = r#"[user]
    name = Test User
    email = test@example.com

[credential]
    helper = osxkeychain

[alias]
    co = checkout
"#;
        let output = strip_gitconfig_secrets(input);
        assert!(output.contains("[user]"));
        assert!(output.contains("name = Test User"));
        assert!(output.contains("[alias]"));
        assert!(output.contains("co = checkout"));
        assert!(!output.contains("[credential]"));
        assert!(!output.contains("helper"));
    }

    #[test]
    fn strip_gitconfig_removes_credential_with_url() {
        let input = r#"[user]
    name = Test User

[credential "https://github.com"]
    helper = !gh auth git-credential
    username = testuser

[core]
    autocrlf = input
"#;
        let output = strip_gitconfig_secrets(input);
        assert!(output.contains("[user]"));
        assert!(output.contains("[core]"));
        assert!(!output.contains("[credential"));
        assert!(!output.contains("helper"));
        assert!(!output.contains("username = testuser"));
    }

    #[test]
    fn strip_gitconfig_preserves_non_credential() {
        let input = r#"[user]
    name = Test User
    email = test@example.com

[core]
    editor = vim
    autocrlf = input

[alias]
    st = status
    co = checkout
"#;
        let output = strip_gitconfig_secrets(input);
        assert_eq!(output.trim(), input.trim());
    }

    #[test]
    fn strip_gitconfig_empty_content() {
        let output = strip_gitconfig_secrets("");
        assert_eq!(output, "");
    }

    #[test]
    fn strip_gitconfig_only_credential_section() {
        let input = r#"[credential]
    helper = store
"#;
        let output = strip_gitconfig_secrets(input);
        assert_eq!(output, "");
    }

    #[test]
    fn strip_gitconfig_multiple_credential_sections() {
        let input = r#"[user]
    name = Test

[credential]
    helper = osxkeychain

[credential "https://github.com"]
    helper = !gh auth git-credential

[core]
    editor = vim
"#;
        let output = strip_gitconfig_secrets(input);
        assert!(output.contains("[user]"));
        assert!(output.contains("[core]"));
        assert!(!output.contains("[credential"));
        assert!(!output.contains("helper"));
    }

    #[test]
    fn strip_gitconfig_credential_with_inline_comment() {
        let input = r#"[user]
    name = Test

[credential] # macOS keychain helper
    helper = osxkeychain

[core]
    editor = vim
"#;
        let output = strip_gitconfig_secrets(input);
        assert!(output.contains("[user]"));
        assert!(output.contains("[core]"));
        assert!(!output.contains("[credential"));
        assert!(!output.contains("osxkeychain"));
    }

    #[test]
    fn strip_gitconfig_credential_url_with_semicolon_comment() {
        let input = r#"[credential "https://github.com"] ; GH credentials
    helper = !gh auth git-credential

[alias]
    co = checkout
"#;
        let output = strip_gitconfig_secrets(input);
        assert!(!output.contains("[credential"));
        assert!(!output.contains("helper"));
        assert!(output.contains("[alias]"));
    }

    #[test]
    fn strip_gitconfig_commented_out_credential_preserved() {
        // A comment that mentions [credential] should not trigger stripping
        let input = r#"[user]
    name = Test
# [credential]
#     helper = osxkeychain
[core]
    editor = vim
"#;
        let output = strip_gitconfig_secrets(input);
        assert!(output.contains("# [credential]"));
        assert!(output.contains("#     helper = osxkeychain"));
    }

    #[test]
    fn is_risky_known_credential_files() {
        for path in RISKY_DOTFILES {
            assert!(is_risky_dotfile(path), "expected '{}' to be risky", path);
        }
        // Also works with full paths
        assert!(is_risky_dotfile("/home/user/.npmrc"));
    }

    #[test]
    fn is_not_risky_safe_dotfiles() {
        assert!(!is_risky_dotfile(".gitconfig"));
        assert!(!is_risky_dotfile(".bashrc"));
        assert!(!is_risky_dotfile(".zshrc"));
    }

    #[test]
    fn prepare_dotfile_strips_gitconfig() {
        let input = r#"[user]
    name = Test

[credential]
    helper = store
"#;
        let output = prepare_dotfile_content(".gitconfig", input);
        assert!(output.contains("[user]"));
        assert!(!output.contains("[credential]"));
    }

    #[test]
    fn prepare_dotfile_strips_gitconfig_with_path() {
        let input = r#"[credential]
    helper = store

[user]
    name = Test
"#;
        let output = prepare_dotfile_content("/home/user/.gitconfig", input);
        assert!(!output.contains("[credential]"));
        assert!(output.contains("[user]"));
    }

    #[test]
    fn prepare_dotfile_passes_through_unknown() {
        let input = "some content\nmore lines\n";
        let output = prepare_dotfile_content(".bashrc", input);
        assert_eq!(output, input);
    }

    #[test]
    fn prepare_dotfile_passes_through_zshrc() {
        // Shell configs are NOT rewritten — symlinks in sandbox home handle $HOME paths
        let input = "source $HOME/.oh-my-zsh/oh-my-zsh.sh\n";
        let output = prepare_dotfile_content(".zshrc", input);
        assert_eq!(output, input);
    }

    #[test]
    fn prepare_dotfile_passes_through_zshenv() {
        let input = "export NVM_DIR=\"$HOME/.nvm\"\n";
        let output = prepare_dotfile_content(".zshenv", input);
        assert_eq!(output, input);
    }

    // ---- copy_claude_config_dir tests ----

    #[tokio::test]
    async fn copy_claude_config_dir_copies_allowlisted_entries() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path();
        let dst = dst_guard.path().join("dest");

        // Create allowlisted files and dirs
        tokio::fs::write(src.join("CLAUDE.md"), b"# Config")
            .await
            .unwrap();
        tokio::fs::write(src.join("settings.json"), b"{}")
            .await
            .unwrap();
        tokio::fs::create_dir_all(src.join("skills").join("review"))
            .await
            .unwrap();
        tokio::fs::write(src.join("skills").join("review").join("skill.md"), b"skill")
            .await
            .unwrap();

        // Non-allowlisted (should be excluded)
        tokio::fs::create_dir_all(src.join("sessions"))
            .await
            .unwrap();
        tokio::fs::write(src.join("sessions").join("big.json"), b"[]")
            .await
            .unwrap();

        let project_dir = std::path::Path::new("/nonexistent/project");
        copy_claude_config_dir(src, &dst, project_dir)
            .await
            .unwrap();

        assert!(dst.join("CLAUDE.md").exists());
        assert!(dst.join("settings.json").exists());
        assert!(dst.join("skills").join("review").join("skill.md").exists());
        assert!(!dst.join("sessions").exists());
    }

    #[tokio::test]
    async fn copy_claude_config_dir_copies_current_project_memory() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path();
        let dst = dst_guard.path().join("dest");

        // Simulate project memory dir
        let project_dir = std::path::PathBuf::from("/Users/test/my-project");
        let project_key = project_dir.to_string_lossy().replace('/', "-");
        let mem_dir = src.join("projects").join(&project_key);
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();
        tokio::fs::write(mem_dir.join("MEMORY.md"), b"# Memory")
            .await
            .unwrap();

        // Another project that should NOT be copied
        let other_dir = src.join("projects").join("-other-project");
        tokio::fs::create_dir_all(&other_dir).await.unwrap();
        tokio::fs::write(other_dir.join("MEMORY.md"), b"# Other")
            .await
            .unwrap();

        copy_claude_config_dir(src, &dst, &project_dir)
            .await
            .unwrap();

        assert!(dst
            .join("projects")
            .join(&project_key)
            .join("MEMORY.md")
            .exists());
        assert!(!dst.join("projects").join("-other-project").exists());
    }

    #[tokio::test]
    async fn copy_claude_config_dir_empty_source_is_ok() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let project_dir = std::path::Path::new("/proj");
        copy_claude_config_dir(
            src_guard.path(),
            &dst_guard.path().join("dest"),
            project_dir,
        )
        .await
        .unwrap();
    }

    /// Unknown top-level entries must not be copied and must not cause an error.
    /// The forward-compatibility scan is informational only.
    #[tokio::test]
    async fn copy_claude_config_dir_ignores_unknown_entries() {
        let src_guard = tempfile::tempdir().unwrap();
        let dst_guard = tempfile::tempdir().unwrap();
        let src = src_guard.path();
        let dst = dst_guard.path().join("dest");

        // One allowlisted entry (should be copied)
        tokio::fs::write(src.join("CLAUDE.md"), b"# Config")
            .await
            .unwrap();

        // Unknown entry — not in CLAUDE_ALLOW_ENTRIES or KNOWN_SKIP_ENTRIES
        tokio::fs::write(src.join("new-future-config.json"), b"{}")
            .await
            .unwrap();
        tokio::fs::create_dir_all(src.join("new-future-dir"))
            .await
            .unwrap();

        let project_dir = std::path::Path::new("/proj");
        // Must succeed — unknown entries are only logged, never an error
        copy_claude_config_dir(src, &dst, project_dir)
            .await
            .unwrap();

        // Allowlisted entry is present
        assert!(dst.join("CLAUDE.md").exists());
        // Unknown entries are NOT copied
        assert!(!dst.join("new-future-config.json").exists());
        assert!(!dst.join("new-future-dir").exists());
    }
}
