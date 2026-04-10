//! Safe dotfile handling for native sandbox
//!
//! Copies dotfiles into the sandbox home directory with secret stripping.
//! Known credential-bearing files have their secrets removed before copying.

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
}
