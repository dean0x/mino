//! Pure helper functions extracted from setup submodules.
//!
//! These are small, testable functions that replace inline patterns
//! across container_linux, container_macos, and native_macos setup flows.

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
        assert_eq!(parse_first_line("podman version 4.9.3"), "podman version 4.9.3");
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
        assert_eq!(generate_subid_entry("_mino_agent"), "_mino_agent:100000:65536");
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
