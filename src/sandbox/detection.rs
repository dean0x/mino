//! Setup detection helpers for `mino setup --native`
//!
//! Constants and functions for detecting toolchain directories, sensitive
//! credential stores, and agent configuration directories on the host system.
//! Used by `src/cli/commands/setup/helpers.rs` to drive interactive setup.

use std::path::Path;

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
