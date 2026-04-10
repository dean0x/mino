//! Safe dotfile handling for native sandbox
//!
//! Copies dotfiles into the sandbox home directory with secret stripping.
//! Known credential-bearing files have their secrets removed before copying.
//! Also contains detection helpers for `mino setup --native`.

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

    Ok(())
}

// =============================================================================
// Setup detection helpers
// =============================================================================

/// Toolchain and shell plugin directories that are safe to passthrough read-only.
///
/// These are commonly referenced from `.zshrc`/`.zshenv` but are NOT credential
/// stores and NOT in `SENSITIVE_PATHS`. Adding them via `mino setup --native` makes
/// the sandbox's shell init noise-free without granting credential access.
pub const TOOLCHAIN_PASSTHROUGH_CANDIDATES: &[&str] = &[
    // Rust
    ".cargo",
    ".rustup",
    // Node.js
    ".nvm",
    ".npm",
    ".yarn",
    ".pnpm",
    ".bun",
    ".volta",
    // Python
    ".pyenv",
    ".pipx",
    ".poetry",
    ".uv",
    ".virtualenvs",
    // Ruby
    ".rbenv",
    ".gem",
    ".bundle",
    // Java/JVM
    ".sdkman",
    ".gradle",
    ".m2",
    // Go (GOPATH default)
    ".go",
    // Haskell
    ".ghcup",
    ".cabal",
    ".stack",
    // OCaml
    ".opam",
    // Dart/Flutter
    ".pub-cache",
    ".flutter",
    // Polyglot version managers
    ".deno",
    ".mise",
    ".asdf",
    // Shell plugin managers / themes
    ".oh-my-zsh",
    ".zsh",
    ".bash_it",
    ".fzf",
    ".starship",
];

/// Sensitive-but-useful directories offered by the interactive setup.
///
/// These are in `SENSITIVE_PATHS` (credential stores) but many AI coding workflows
/// require them. The setup flow presents them separately with an explicit warning,
/// and writes accepted entries into BOTH `auto_passthrough_dirs` AND
/// `allow_sensitive_paths` so that `SandboxConfig::validate()` allows them.
pub const SENSITIVE_BUT_USEFUL_CANDIDATES: &[&str] = &[".config/gh", ".docker", ".kube"];

/// The single directory offered for auto-copy (agent memory).
pub const CLAUDE_AUTO_COPY_CANDIDATE: &str = ".claude";

/// Detect which toolchain passthrough candidates exist on the host.
///
/// For each candidate in `candidates`, returns those where `home.join(candidate)`
/// exists AND is a directory. Entries that appear in `sensitive_blocklist` are
/// silently filtered out even if they somehow ended up in `candidates`.
/// The order of the returned vec matches the order of `candidates`.
pub fn detect_passthrough_candidates(
    home: &Path,
    candidates: &[&str],
    sensitive_blocklist: &[&str],
) -> Vec<String> {
    candidates
        .iter()
        .filter(|&&c| !sensitive_blocklist.contains(&c))
        .filter(|&&c| home.join(c).is_dir())
        .map(|&c| c.to_string())
        .collect()
}

/// Detect which sensitive-but-useful candidates exist on the host.
///
/// Returns those entries from `sensitive_but_useful` where `home.join(entry)`
/// exists AND is a directory. Returned in the same order as the input.
///
/// Sensitive candidates are not filtered against a blocklist — the caller
/// is responsible for presenting them with an appropriate warning.
pub fn detect_sensitive_candidates(home: &Path, sensitive_but_useful: &[&str]) -> Vec<String> {
    detect_passthrough_candidates(home, sensitive_but_useful, &[])
}

/// Detect whether `.claude` exists on the host and should be offered for auto-copy.
///
/// Returns `Some(".claude".into())` iff `home.join(".claude")` exists as a directory.
pub fn detect_claude_copy_candidate(home: &Path) -> Option<String> {
    home.join(CLAUDE_AUTO_COPY_CANDIDATE)
        .is_dir()
        .then(|| CLAUDE_AUTO_COPY_CANDIDATE.to_string())
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

    // ---- detection helper tests ----

    #[test]
    fn detect_passthrough_candidates_finds_existing_dirs() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".cargo")).unwrap();
        std::fs::create_dir(home.path().join(".nvm")).unwrap();

        let candidates = &[".cargo", ".nvm", ".oh-my-zsh"];
        let result = detect_passthrough_candidates(
            home.path(),
            candidates,
            crate::sandbox::config::SENSITIVE_PATHS,
        );
        assert_eq!(result, vec![".cargo".to_string(), ".nvm".to_string()]);
    }

    #[test]
    fn detect_passthrough_candidates_skips_missing() {
        let home = tempfile::tempdir().unwrap();
        // Create only .cargo
        std::fs::create_dir(home.path().join(".cargo")).unwrap();

        let candidates = &[".cargo", ".nvm"];
        let result = detect_passthrough_candidates(
            home.path(),
            candidates,
            crate::sandbox::config::SENSITIVE_PATHS,
        );
        assert_eq!(result, vec![".cargo".to_string()]);
    }

    #[test]
    fn detect_passthrough_candidates_ignores_files() {
        let home = tempfile::tempdir().unwrap();
        // .cargo exists as a regular file, NOT a directory
        std::fs::write(home.path().join(".cargo"), "not a dir").unwrap();

        let candidates = &[".cargo"];
        let result = detect_passthrough_candidates(
            home.path(),
            candidates,
            crate::sandbox::config::SENSITIVE_PATHS,
        );
        assert!(result.is_empty(), "regular files should not be returned");
    }

    #[test]
    fn detect_passthrough_candidates_filters_blocklist() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".docker")).unwrap();
        std::fs::create_dir(home.path().join(".cargo")).unwrap();

        // .docker is in SENSITIVE_PATHS — even if passed as a candidate it must be filtered
        let candidates = &[".cargo", ".docker"];
        let blocklist = &[".docker"];
        let result = detect_passthrough_candidates(home.path(), candidates, blocklist);
        assert_eq!(result, vec![".cargo".to_string()]);
        assert!(!result.iter().any(|s| s == ".docker"));
    }

    #[test]
    fn detect_passthrough_candidates_preserves_order() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".cargo")).unwrap();
        std::fs::create_dir(home.path().join(".nvm")).unwrap();
        std::fs::create_dir(home.path().join(".pyenv")).unwrap();

        let candidates = &[".cargo", ".nvm", ".pyenv"];
        let result = detect_passthrough_candidates(home.path(), candidates, &[]);
        assert_eq!(
            result,
            vec![
                ".cargo".to_string(),
                ".nvm".to_string(),
                ".pyenv".to_string()
            ]
        );
    }

    #[test]
    fn detect_sensitive_candidates_present_and_absent() {
        let home = tempfile::tempdir().unwrap();
        // Create .config/gh but not .docker or .kube
        std::fs::create_dir_all(home.path().join(".config").join("gh")).unwrap();

        let result = detect_sensitive_candidates(home.path(), SENSITIVE_BUT_USEFUL_CANDIDATES);
        assert_eq!(result, vec![".config/gh".to_string()]);
    }

    #[test]
    fn detect_sensitive_candidates_empty_when_none_present() {
        let home = tempfile::tempdir().unwrap();
        let result = detect_sensitive_candidates(home.path(), SENSITIVE_BUT_USEFUL_CANDIDATES);
        assert!(result.is_empty());
    }

    #[test]
    fn detect_claude_copy_candidate_present() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".claude")).unwrap();
        let result = detect_claude_copy_candidate(home.path());
        assert_eq!(result, Some(".claude".to_string()));
    }

    #[test]
    fn detect_claude_copy_candidate_absent() {
        let home = tempfile::tempdir().unwrap();
        let result = detect_claude_copy_candidate(home.path());
        assert!(result.is_none());
    }
}
