//! Network isolation for container sessions
//!
//! Supports three modes: host, none, bridge.
//! With `--network-allow`, uses bridge networking + iptables egress filtering.

use crate::error::{MinoError, MinoResult};

/// A single network allowlist rule: host:port
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRule {
    pub host: String,
    pub port: u16,
}

/// Network mode for container sessions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkMode {
    /// Full host networking (default, backward compatible)
    Host,
    /// No network access
    None,
    /// Bridge networking (outbound allowed)
    Bridge,
    /// Bridge networking with iptables egress allowlist
    Allow(Vec<NetworkRule>),
}

/// Parse a `host:port` string into a `NetworkRule`.
///
/// Supports IPv6 addresses in brackets: `[::1]:443`.
/// Port must be 1-65535. Host must not be empty.
pub fn parse_network_rule(s: &str) -> MinoResult<NetworkRule> {
    let s = s.trim();

    let (host, port_str) = if s.starts_with('[') {
        // IPv6 in brackets: [::1]:443
        let close_bracket = s.find(']').ok_or_else(|| {
            MinoError::NetworkPolicy(format!("Missing closing bracket in IPv6 address: {}", s))
        })?;
        let host = &s[1..close_bracket];
        let rest = &s[close_bracket + 1..];
        if !rest.starts_with(':') {
            return Err(MinoError::NetworkPolicy(format!(
                "Expected ':' after closing bracket in '{}'. Format: [host]:port",
                s
            )));
        }
        (host.to_string(), &rest[1..])
    } else {
        // Regular host:port — split on the last colon to handle IPv4 and hostnames
        let last_colon = s.rfind(':').ok_or_else(|| {
            MinoError::NetworkPolicy(format!(
                "Invalid network rule '{}'. Expected format: host:port",
                s
            ))
        })?;
        (s[..last_colon].to_string(), &s[last_colon + 1..])
    };

    if host.is_empty() {
        return Err(MinoError::NetworkPolicy(
            "Empty host in network rule".to_string(),
        ));
    }

    let port: u16 = port_str.parse().map_err(|_| {
        MinoError::NetworkPolicy(format!(
            "Invalid port '{}' in network rule '{}'. Must be 1-65535",
            port_str, s
        ))
    })?;

    if port == 0 {
        return Err(MinoError::NetworkPolicy(format!(
            "Port 0 is not valid in network rule '{}'. Must be 1-65535",
            s
        )));
    }

    Ok(NetworkRule { host, port })
}

/// Parse a mode string ("host", "none", "bridge") into a `NetworkMode`.
///
/// `source` is included in error messages for context (e.g. "CLI", "config").
fn parse_mode_str(s: &str, source: &str) -> MinoResult<NetworkMode> {
    match s {
        "none" => Ok(NetworkMode::None),
        "bridge" => Ok(NetworkMode::Bridge),
        "host" => Ok(NetworkMode::Host),
        other => Err(MinoError::NetworkPolicy(format!(
            "Unknown network mode '{}' in {}. Valid modes: host, none, bridge",
            other, source
        ))),
    }
}

/// Parse a slice of `host:port` strings into `NetworkRule`s.
fn parse_rules(raw: &[String]) -> MinoResult<Vec<NetworkRule>> {
    raw.iter()
        .map(|r| parse_network_rule(r))
        .collect()
}

/// Resolve the effective network mode from CLI flags and config values.
///
/// Precedence:
/// 1. CLI `--network-allow` (non-empty) implies bridge + iptables allowlist.
/// 2. CLI `--network` overrides config.
/// 3. Config `network_allow` (non-empty) implies bridge + iptables allowlist.
/// 4. Config `network` as fallback.
pub fn resolve_network_mode(
    cli_network: Option<&str>,
    cli_allow_rules: &[String],
    config_network: &str,
    config_network_allow: &[String],
) -> MinoResult<NetworkMode> {
    // CLI allow rules take highest precedence
    if !cli_allow_rules.is_empty() {
        // Conflict: --network none + --network-allow
        if cli_network == Some("none") {
            return Err(MinoError::NetworkPolicy(
                "Cannot combine --network none with --network-allow. \
                 Allowlist rules require bridge networking."
                    .to_string(),
            ));
        }

        // Override: --network host + --network-allow (warn but proceed)
        if cli_network == Some("host") {
            tracing::warn!(
                "--network host overridden to bridge because --network-allow was specified"
            );
        }

        return Ok(NetworkMode::Allow(parse_rules(cli_allow_rules)?));
    }

    // CLI --network flag (without allow rules)
    if let Some(net) = cli_network {
        return parse_mode_str(net, "CLI");
    }

    // Config allow rules (no CLI override)
    if !config_network_allow.is_empty() {
        // Conflict: config network = "none" with network_allow entries
        if config_network == "none" {
            return Err(MinoError::NetworkPolicy(
                "Config conflict: network = \"none\" with network_allow entries. \
                 Allowlist rules require bridge networking."
                    .to_string(),
            ));
        }

        return Ok(NetworkMode::Allow(parse_rules(config_network_allow)?));
    }

    // Config network mode fallback
    parse_mode_str(config_network, "config")
}

impl NetworkMode {
    /// Returns the Podman `--network` flag value.
    pub fn to_podman_network(&self) -> &str {
        match self {
            NetworkMode::Host => "host",
            NetworkMode::None => "none",
            NetworkMode::Bridge | NetworkMode::Allow(_) => "bridge",
        }
    }

    /// Whether the container needs `CAP_NET_ADMIN` for iptables.
    pub fn requires_cap_net_admin(&self) -> bool {
        matches!(self, NetworkMode::Allow(_))
    }
}

/// POSIX single-quote escaping: replace `'` with `'\''`.
pub fn shell_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Generate an iptables wrapper that enforces egress allowlist rules,
/// then `exec`s the original command.
///
/// Returns a command vector: `["/bin/sh", "-c", "<script>"]`.
pub fn generate_iptables_wrapper(rules: &[NetworkRule], original_command: &[String]) -> Vec<String> {
    let mut script = String::from("set -e; ");

    // Verify iptables is available before attempting network filtering
    script.push_str(
        "command -v iptables >/dev/null 2>&1 || { echo 'mino: iptables not found in container image. \
         Network allowlist requires iptables.' >&2; exit 1; }; ",
    );

    // Drop all outbound traffic by default (IPv4 + IPv6)
    script.push_str("iptables -P OUTPUT DROP; ");
    script.push_str("ip6tables -P OUTPUT DROP; ");

    // Allow loopback
    script.push_str("iptables -A OUTPUT -o lo -j ACCEPT; ");
    script.push_str("ip6tables -A OUTPUT -o lo -j ACCEPT; ");

    // Allow established/related connections (IPv4)
    script.push_str("iptables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT; ");
    // Allow DNS (IPv4)
    script.push_str("iptables -A OUTPUT -p udp --dport 53 -j ACCEPT; ");
    script.push_str("iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT; ");

    // Allow established/related connections (IPv6)
    script.push_str("ip6tables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT; ");
    // Allow DNS (IPv6)
    script.push_str("ip6tables -A OUTPUT -p udp --dport 53 -j ACCEPT; ");
    script.push_str("ip6tables -A OUTPUT -p tcp --dport 53 -j ACCEPT; ");

    // Add allowlist rules (both IPv4 and IPv6 for each destination)
    for rule in rules {
        let escaped_host = shell_escape(&rule.host);
        script.push_str(&format!(
            "iptables -A OUTPUT -d '{}' -p tcp --dport {} -j ACCEPT; ",
            escaped_host, rule.port
        ));
        script.push_str(&format!(
            "ip6tables -A OUTPUT -d '{}' -p tcp --dport {} -j ACCEPT; ",
            escaped_host, rule.port
        ));
    }

    // Exec the original command
    script.push_str("exec");
    for arg in original_command {
        script.push_str(&format!(" '{}'", shell_escape(arg)));
    }

    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        script,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_network_rule tests ----

    #[test]
    fn parse_valid_host_port() {
        let rule = parse_network_rule("github.com:443").unwrap();
        assert_eq!(rule.host, "github.com");
        assert_eq!(rule.port, 443);
    }

    #[test]
    fn parse_valid_ip_port() {
        let rule = parse_network_rule("192.168.1.1:8080").unwrap();
        assert_eq!(rule.host, "192.168.1.1");
        assert_eq!(rule.port, 8080);
    }

    #[test]
    fn parse_ipv6_bracketed() {
        let rule = parse_network_rule("[::1]:443").unwrap();
        assert_eq!(rule.host, "::1");
        assert_eq!(rule.port, 443);
    }

    #[test]
    fn parse_ipv6_full_bracketed() {
        let rule = parse_network_rule("[2001:db8::1]:8080").unwrap();
        assert_eq!(rule.host, "2001:db8::1");
        assert_eq!(rule.port, 8080);
    }

    #[test]
    fn parse_trims_whitespace() {
        let rule = parse_network_rule("  github.com:443  ").unwrap();
        assert_eq!(rule.host, "github.com");
        assert_eq!(rule.port, 443);
    }

    #[test]
    fn parse_port_1() {
        let rule = parse_network_rule("host:1").unwrap();
        assert_eq!(rule.port, 1);
    }

    #[test]
    fn parse_port_max() {
        let rule = parse_network_rule("host:65535").unwrap();
        assert_eq!(rule.port, 65535);
    }

    #[test]
    fn parse_empty_host_rejected() {
        let result = parse_network_rule(":443");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Empty host"));
    }

    #[test]
    fn parse_port_zero_rejected() {
        let result = parse_network_rule("host:0");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not valid"));
    }

    #[test]
    fn parse_port_too_large_rejected() {
        let result = parse_network_rule("host:70000");
        assert!(result.is_err());
    }

    #[test]
    fn parse_missing_port_rejected() {
        let result = parse_network_rule("github.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("host:port"));
    }

    #[test]
    fn parse_invalid_port_string_rejected() {
        let result = parse_network_rule("host:abc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid port"));
    }

    #[test]
    fn parse_empty_string_rejected() {
        let result = parse_network_rule("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_ipv6_missing_close_bracket() {
        let result = parse_network_rule("[::1:443");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("closing bracket"));
    }

    #[test]
    fn parse_ipv6_missing_port_after_bracket() {
        let result = parse_network_rule("[::1]");
        assert!(result.is_err());
    }

    #[test]
    fn parse_ipv6_empty_host_in_brackets() {
        let result = parse_network_rule("[]:443");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Empty host"));
    }

    // ---- resolve_network_mode tests ----

    #[test]
    fn resolve_defaults_to_config_host() {
        let mode = resolve_network_mode(None, &[], "host", &[]).unwrap();
        assert_eq!(mode, NetworkMode::Host);
    }

    #[test]
    fn resolve_defaults_to_config_none() {
        let mode = resolve_network_mode(None, &[], "none", &[]).unwrap();
        assert_eq!(mode, NetworkMode::None);
    }

    #[test]
    fn resolve_defaults_to_config_bridge() {
        let mode = resolve_network_mode(None, &[], "bridge", &[]).unwrap();
        assert_eq!(mode, NetworkMode::Bridge);
    }

    #[test]
    fn resolve_cli_network_overrides_config() {
        let mode = resolve_network_mode(Some("none"), &[], "host", &[]).unwrap();
        assert_eq!(mode, NetworkMode::None);
    }

    #[test]
    fn resolve_cli_bridge() {
        let mode = resolve_network_mode(Some("bridge"), &[], "host", &[]).unwrap();
        assert_eq!(mode, NetworkMode::Bridge);
    }

    #[test]
    fn resolve_cli_allow_implies_bridge() {
        let mode = resolve_network_mode(
            None,
            &["github.com:443".to_string()],
            "host",
            &[],
        )
        .unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].host, "github.com");
                assert_eq!(rules[0].port, 443);
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_allow_multiple_rules() {
        let mode = resolve_network_mode(
            None,
            &[
                "github.com:443".to_string(),
                "npmjs.org:443".to_string(),
            ],
            "host",
            &[],
        )
        .unwrap();
        match mode {
            NetworkMode::Allow(rules) => assert_eq!(rules.len(), 2),
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_none_with_allow_is_error() {
        let result = resolve_network_mode(
            Some("none"),
            &["github.com:443".to_string()],
            "host",
            &[],
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot combine"));
    }

    #[test]
    fn resolve_cli_host_with_allow_overrides_to_allow() {
        let mode = resolve_network_mode(
            Some("host"),
            &["github.com:443".to_string()],
            "host",
            &[],
        )
        .unwrap();
        assert!(matches!(mode, NetworkMode::Allow(_)));
    }

    #[test]
    fn resolve_config_allow_rules() {
        let mode = resolve_network_mode(
            None,
            &[],
            "host",
            &["registry.npmjs.org:443".to_string()],
        )
        .unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].host, "registry.npmjs.org");
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_allow_overrides_config_allow() {
        let mode = resolve_network_mode(
            None,
            &["github.com:443".to_string()],
            "host",
            &["npmjs.org:443".to_string()],
        )
        .unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].host, "github.com");
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_unknown_mode_error() {
        let result = resolve_network_mode(Some("invalid"), &[], "host", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown network mode"));
    }

    #[test]
    fn resolve_config_none_with_allow_is_error() {
        let result = resolve_network_mode(
            None,
            &[],
            "none",
            &["github.com:443".to_string()],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Config conflict"));
    }

    #[test]
    fn resolve_unknown_config_mode_error() {
        let result = resolve_network_mode(None, &[], "invalid", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown network mode"));
    }

    // ---- NetworkMode method tests ----

    #[test]
    fn to_podman_network_host() {
        assert_eq!(NetworkMode::Host.to_podman_network(), "host");
    }

    #[test]
    fn to_podman_network_none() {
        assert_eq!(NetworkMode::None.to_podman_network(), "none");
    }

    #[test]
    fn to_podman_network_bridge() {
        assert_eq!(NetworkMode::Bridge.to_podman_network(), "bridge");
    }

    #[test]
    fn to_podman_network_allow_is_bridge() {
        let mode = NetworkMode::Allow(vec![NetworkRule {
            host: "x".to_string(),
            port: 443,
        }]);
        assert_eq!(mode.to_podman_network(), "bridge");
    }

    #[test]
    fn requires_cap_net_admin_only_for_allow() {
        assert!(!NetworkMode::Host.requires_cap_net_admin());
        assert!(!NetworkMode::None.requires_cap_net_admin());
        assert!(!NetworkMode::Bridge.requires_cap_net_admin());
        assert!(NetworkMode::Allow(vec![]).requires_cap_net_admin());
    }

    // ---- shell_escape tests ----

    #[test]
    fn shell_escape_no_quotes() {
        assert_eq!(shell_escape("hello"), "hello");
    }

    #[test]
    fn shell_escape_single_quote() {
        assert_eq!(shell_escape("it's"), "it'\\''s");
    }

    #[test]
    fn shell_escape_multiple_quotes() {
        assert_eq!(shell_escape("a'b'c"), "a'\\''b'\\''c");
    }

    #[test]
    fn shell_escape_empty() {
        assert_eq!(shell_escape(""), "");
    }

    #[test]
    fn shell_escape_only_quotes() {
        assert_eq!(shell_escape("'''"), "'\\'''\\'''\\''");
    }

    // ---- generate_iptables_wrapper tests ----

    #[test]
    fn iptables_wrapper_basic() {
        let rules = vec![NetworkRule {
            host: "github.com".to_string(),
            port: 443,
        }];
        let cmd = vec!["bash".to_string()];
        let result = generate_iptables_wrapper(&rules, &cmd);

        assert_eq!(result[0], "/bin/sh");
        assert_eq!(result[1], "-c");

        let script = &result[2];
        assert!(script.starts_with("set -e; "));
        assert!(script.contains("iptables -P OUTPUT DROP"));
        assert!(script.contains("ip6tables -P OUTPUT DROP"));
        assert!(script.contains("iptables -A OUTPUT -o lo -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -o lo -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -p udp --dport 53 -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -p udp --dport 53 -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -p tcp --dport 53 -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -d 'github.com' -p tcp --dport 443 -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -d 'github.com' -p tcp --dport 443 -j ACCEPT"));
        assert!(script.contains("command -v iptables"));
        assert!(script.ends_with("exec 'bash'"));
    }

    #[test]
    fn iptables_wrapper_multiple_rules() {
        let rules = vec![
            NetworkRule {
                host: "github.com".to_string(),
                port: 443,
            },
            NetworkRule {
                host: "npmjs.org".to_string(),
                port: 443,
            },
        ];
        let cmd = vec!["node".to_string(), "app.js".to_string()];
        let result = generate_iptables_wrapper(&rules, &cmd);
        let script = &result[2];

        assert!(script.contains("iptables -A OUTPUT -d 'github.com' -p tcp --dport 443"));
        assert!(script.contains("ip6tables -A OUTPUT -d 'github.com' -p tcp --dport 443"));
        assert!(script.contains("iptables -A OUTPUT -d 'npmjs.org' -p tcp --dport 443"));
        assert!(script.contains("ip6tables -A OUTPUT -d 'npmjs.org' -p tcp --dport 443"));
        assert!(script.ends_with("exec 'node' 'app.js'"));
    }

    #[test]
    fn iptables_wrapper_escapes_single_quotes_in_command() {
        let rules = vec![];
        let cmd = vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo 'hello world'".to_string(),
        ];
        let result = generate_iptables_wrapper(&rules, &cmd);
        let script = &result[2];

        // The command arg with quotes should be escaped
        assert!(script.contains("echo '\\''hello world'\\''"));
    }

    #[test]
    fn iptables_wrapper_escapes_host_with_single_quote() {
        let rules = vec![NetworkRule {
            host: "host'name".to_string(),
            port: 443,
        }];
        let cmd = vec!["bash".to_string()];
        let result = generate_iptables_wrapper(&rules, &cmd);
        let script = &result[2];

        assert!(script.contains("iptables -A OUTPUT -d 'host'\\''name' -p tcp --dport 443"));
        assert!(script.contains("ip6tables -A OUTPUT -d 'host'\\''name' -p tcp --dport 443"));
    }

    #[test]
    fn iptables_wrapper_empty_rules() {
        let rules = vec![];
        let cmd = vec!["bash".to_string()];
        let result = generate_iptables_wrapper(&rules, &cmd);
        let script = &result[2];

        // Should still have base rules (DROP, loopback, DNS) but no allowlist entries
        assert!(script.contains("iptables -P OUTPUT DROP"));
        assert!(!script.contains("-d '"));
        assert!(script.ends_with("exec 'bash'"));
    }

    #[test]
    fn iptables_wrapper_multi_word_command() {
        let rules = vec![];
        let cmd = vec![
            "/bin/bash".to_string(),
            "-c".to_string(),
            "ls -la".to_string(),
        ];
        let result = generate_iptables_wrapper(&rules, &cmd);
        let script = &result[2];

        assert!(script.ends_with("exec '/bin/bash' '-c' 'ls -la'"));
    }
}
