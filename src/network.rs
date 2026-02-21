//! Network isolation for container sessions
//!
//! Supports four modes: host, none, bridge, and allow (bridge + iptables egress filtering).
//! Includes preset resolution for common allowlist configurations.

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
    /// Full host networking (shares host network namespace)
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
    raw.iter().map(|r| parse_network_rule(r)).collect()
}

/// Resolve a network preset name into a list of `NetworkRule`s.
///
/// Built-in presets:
/// - `dev`: GitHub, npm, crates.io, PyPI, AI APIs
/// - `registries`: Package registries only
pub fn resolve_preset(name: &str) -> MinoResult<Vec<NetworkRule>> {
    let rules: Vec<(&str, u16)> = match name {
        "dev" => vec![
            ("github.com", 443),
            ("github.com", 22),
            ("api.github.com", 443),
            ("registry.npmjs.org", 443),
            ("crates.io", 443),
            ("static.crates.io", 443),
            ("index.crates.io", 443),
            ("pypi.org", 443),
            ("files.pythonhosted.org", 443),
            ("api.anthropic.com", 443),
            ("api.openai.com", 443),
        ],
        "registries" => vec![
            ("registry.npmjs.org", 443),
            ("crates.io", 443),
            ("static.crates.io", 443),
            ("index.crates.io", 443),
            ("pypi.org", 443),
            ("files.pythonhosted.org", 443),
        ],
        other => {
            return Err(MinoError::NetworkPolicy(format!(
                "Unknown network preset '{}'. Available presets: dev, registries",
                other
            )));
        }
    };

    Ok(rules
        .into_iter()
        .map(|(host, port)| NetworkRule {
            host: host.to_string(),
            port,
        })
        .collect())
}

/// Input for network mode resolution, grouping CLI and config parameters.
pub struct NetworkResolutionInput<'a> {
    pub cli_network: Option<&'a str>,
    pub cli_allow_rules: &'a [String],
    pub cli_preset: Option<&'a str>,
    pub config_network: &'a str,
    pub config_network_allow: &'a [String],
    pub config_preset: Option<&'a str>,
}

/// Resolve the effective network mode from CLI flags, presets, and config values.
///
/// Precedence:
/// 1. CLI `--network-allow` (non-empty) implies bridge + iptables allowlist.
/// 2. CLI `--network-preset` resolves preset into allowlist rules.
/// 3. CLI `--network` overrides config.
/// 4. Config `network_allow` (non-empty) implies bridge + iptables allowlist.
/// 5. Config `network_preset` resolves preset into allowlist rules.
/// 6. Config `network` as fallback.
pub fn resolve_network_mode(input: &NetworkResolutionInput) -> MinoResult<NetworkMode> {
    let NetworkResolutionInput {
        cli_network,
        cli_allow_rules,
        cli_preset,
        config_network,
        config_network_allow,
        config_preset,
    } = input;

    // CLI allow rules take highest precedence
    if !cli_allow_rules.is_empty() {
        // Conflict: --network none + --network-allow
        if *cli_network == Some("none") {
            return Err(MinoError::NetworkPolicy(
                "Cannot combine --network none with --network-allow. \
                 Allowlist rules require bridge networking."
                    .to_string(),
            ));
        }

        // Override: --network host + --network-allow (warn but proceed)
        if *cli_network == Some("host") {
            tracing::warn!(
                "--network host overridden to bridge because --network-allow was specified"
            );
        }

        return Ok(NetworkMode::Allow(parse_rules(cli_allow_rules)?));
    }

    // CLI --network-preset
    if let Some(preset) = cli_preset {
        if *cli_network == Some("none") {
            return Err(MinoError::NetworkPolicy(
                "Cannot combine --network none with --network-preset. \
                 Presets require bridge networking."
                    .to_string(),
            ));
        }
        if *cli_network == Some("host") {
            tracing::warn!(
                "--network host overridden to bridge because --network-preset was specified"
            );
        }
        return Ok(NetworkMode::Allow(resolve_preset(preset)?));
    }

    // CLI --network flag (without allow rules or preset)
    if let Some(net) = *cli_network {
        return parse_mode_str(net, "CLI");
    }

    // Config allow rules (no CLI override)
    if !config_network_allow.is_empty() {
        // Conflict: config network = "none" with network_allow entries
        if *config_network == "none" {
            return Err(MinoError::NetworkPolicy(
                "Config conflict: network = \"none\" with network_allow entries. \
                 Allowlist rules require bridge networking."
                    .to_string(),
            ));
        }

        return Ok(NetworkMode::Allow(parse_rules(config_network_allow)?));
    }

    // Config network_preset
    if let Some(preset) = config_preset {
        if *config_network == "none" {
            return Err(MinoError::NetworkPolicy(
                "Config conflict: network = \"none\" with network_preset. \
                 Presets require bridge networking."
                    .to_string(),
            ));
        }
        return Ok(NetworkMode::Allow(resolve_preset(preset)?));
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
pub fn generate_iptables_wrapper(
    rules: &[NetworkRule],
    original_command: &[String],
) -> Vec<String> {
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

    // Drop CAP_NET_ADMIN before exec'ing the user command.
    // The capsh -- -c 'exec "$@"' -- arg1 arg2 pattern passes args as
    // positional parameters, avoiding nested quoting issues.
    // If capsh is not available, abort — running with CAP_NET_ADMIN would let
    // the agent flush iptables rules and bypass the allowlist.
    let mut escaped_args = String::new();
    for arg in original_command {
        escaped_args.push_str(&format!(" '{}'", shell_escape(arg)));
    }
    script.push_str(&format!(
        "if command -v capsh >/dev/null 2>&1; then exec capsh --drop=cap_net_admin -- -c 'exec \"$@\"' --{}; \
         else echo 'mino: capsh not found. Cannot drop CAP_NET_ADMIN -- network allowlist is bypassable without it.' >&2; exit 1; fi",
        escaped_args
    ));

    vec!["/bin/sh".to_string(), "-c".to_string(), script]
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
        assert!(result.unwrap_err().to_string().contains("closing bracket"));
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

    // ---- resolve_preset tests ----

    #[test]
    fn resolve_preset_dev() {
        let rules = resolve_preset("dev").unwrap();
        assert!(rules.len() >= 10);
        assert!(rules
            .iter()
            .any(|r| r.host == "github.com" && r.port == 443));
        assert!(rules.iter().any(|r| r.host == "github.com" && r.port == 22));
        assert!(rules
            .iter()
            .any(|r| r.host == "registry.npmjs.org" && r.port == 443));
        assert!(rules
            .iter()
            .any(|r| r.host == "api.anthropic.com" && r.port == 443));
    }

    #[test]
    fn resolve_preset_registries() {
        let rules = resolve_preset("registries").unwrap();
        assert!(rules.len() >= 5);
        assert!(rules
            .iter()
            .any(|r| r.host == "registry.npmjs.org" && r.port == 443));
        assert!(rules.iter().any(|r| r.host == "crates.io" && r.port == 443));
        // Should NOT include GitHub or AI APIs
        assert!(!rules.iter().any(|r| r.host == "github.com"));
        assert!(!rules.iter().any(|r| r.host == "api.anthropic.com"));
    }

    #[test]
    fn resolve_preset_unknown_error() {
        let result = resolve_preset("unknown");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown network preset"));
    }

    // ---- resolve_network_mode tests ----

    /// Helper to build a `NetworkResolutionInput` with defaults for concise tests.
    fn resolve(
        cli_network: Option<&str>,
        cli_allow_rules: &[String],
        cli_preset: Option<&str>,
        config_network: &str,
        config_network_allow: &[String],
        config_preset: Option<&str>,
    ) -> MinoResult<NetworkMode> {
        resolve_network_mode(&NetworkResolutionInput {
            cli_network,
            cli_allow_rules,
            cli_preset,
            config_network,
            config_network_allow,
            config_preset,
        })
    }

    #[test]
    fn resolve_defaults_to_config_host() {
        let mode = resolve(None, &[], None, "host", &[], None).unwrap();
        assert_eq!(mode, NetworkMode::Host);
    }

    #[test]
    fn resolve_defaults_to_config_none() {
        let mode = resolve(None, &[], None, "none", &[], None).unwrap();
        assert_eq!(mode, NetworkMode::None);
    }

    #[test]
    fn resolve_defaults_to_config_bridge() {
        let mode = resolve(None, &[], None, "bridge", &[], None).unwrap();
        assert_eq!(mode, NetworkMode::Bridge);
    }

    #[test]
    fn resolve_cli_network_overrides_config() {
        let mode = resolve(Some("none"), &[], None, "host", &[], None).unwrap();
        assert_eq!(mode, NetworkMode::None);
    }

    #[test]
    fn resolve_cli_bridge() {
        let mode = resolve(Some("bridge"), &[], None, "host", &[], None).unwrap();
        assert_eq!(mode, NetworkMode::Bridge);
    }

    #[test]
    fn resolve_cli_allow_implies_bridge() {
        let mode = resolve(
            None,
            &["github.com:443".to_string()],
            None,
            "host",
            &[],
            None,
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
        let mode = resolve(
            None,
            &["github.com:443".to_string(), "npmjs.org:443".to_string()],
            None,
            "host",
            &[],
            None,
        )
        .unwrap();
        match mode {
            NetworkMode::Allow(rules) => assert_eq!(rules.len(), 2),
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_none_with_allow_is_error() {
        let result = resolve(
            Some("none"),
            &["github.com:443".to_string()],
            None,
            "host",
            &[],
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot combine"));
    }

    #[test]
    fn resolve_cli_host_with_allow_overrides_to_allow() {
        let mode = resolve(
            Some("host"),
            &["github.com:443".to_string()],
            None,
            "host",
            &[],
            None,
        )
        .unwrap();
        assert!(matches!(mode, NetworkMode::Allow(_)));
    }

    #[test]
    fn resolve_config_allow_rules() {
        let mode = resolve(
            None,
            &[],
            None,
            "host",
            &["registry.npmjs.org:443".to_string()],
            None,
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
        let mode = resolve(
            None,
            &["github.com:443".to_string()],
            None,
            "host",
            &["npmjs.org:443".to_string()],
            None,
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
        let result = resolve(Some("invalid"), &[], None, "host", &[], None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown network mode"));
    }

    #[test]
    fn resolve_config_none_with_allow_is_error() {
        let result = resolve(
            None,
            &[],
            None,
            "none",
            &["github.com:443".to_string()],
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Config conflict"));
    }

    #[test]
    fn resolve_unknown_config_mode_error() {
        let result = resolve(None, &[], None, "invalid", &[], None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown network mode"));
    }

    #[test]
    fn resolve_cli_preset_dev() {
        let mode = resolve(None, &[], Some("dev"), "bridge", &[], None).unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                assert!(rules.len() >= 10);
                assert!(rules.iter().any(|r| r.host == "github.com"));
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_preset_overrides_config_preset() {
        let mode = resolve(None, &[], Some("registries"), "bridge", &[], Some("dev")).unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                // registries preset should NOT have github.com
                assert!(!rules.iter().any(|r| r.host == "github.com"));
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_allow_overrides_cli_preset() {
        let mode = resolve(
            None,
            &["custom.host:8080".to_string()],
            Some("dev"),
            "bridge",
            &[],
            None,
        )
        .unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].host, "custom.host");
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_config_preset() {
        let mode = resolve(None, &[], None, "bridge", &[], Some("registries")).unwrap();
        match mode {
            NetworkMode::Allow(rules) => {
                assert!(rules.iter().any(|r| r.host == "crates.io"));
            }
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn resolve_cli_none_with_preset_is_error() {
        let result = resolve(Some("none"), &[], Some("dev"), "bridge", &[], None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot combine"));
    }

    #[test]
    fn resolve_config_none_with_preset_is_error() {
        let result = resolve(None, &[], None, "none", &[], Some("dev"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Config conflict"));
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
        assert!(script
            .contains("iptables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT"));
        assert!(script
            .contains("ip6tables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -p udp --dport 53 -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -p udp --dport 53 -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -p tcp --dport 53 -j ACCEPT"));
        assert!(script.contains("iptables -A OUTPUT -d 'github.com' -p tcp --dport 443 -j ACCEPT"));
        assert!(script.contains("ip6tables -A OUTPUT -d 'github.com' -p tcp --dport 443 -j ACCEPT"));
        assert!(script.contains("command -v iptables"));
        // capsh drop + hard fail if capsh missing
        assert!(script.contains("capsh --drop=cap_net_admin"));
        assert!(script.contains("else echo 'mino: capsh not found"));
        assert!(script.contains("exit 1; fi"));
    }

    #[test]
    fn iptables_wrapper_capsh_drops_cap_net_admin() {
        let rules = vec![NetworkRule {
            host: "github.com".to_string(),
            port: 443,
        }];
        let cmd = vec!["/bin/zsh".to_string()];
        let result = generate_iptables_wrapper(&rules, &cmd);
        let script = &result[2];

        // capsh branch: drops CAP_NET_ADMIN and execs the command
        assert!(
            script.contains("exec capsh --drop=cap_net_admin -- -c 'exec \"$@\"' -- '/bin/zsh'")
        );
        // fallback branch: hard fail when capsh is missing
        assert!(script.contains("else echo 'mino: capsh not found. Cannot drop CAP_NET_ADMIN"));
        assert!(script.contains("exit 1; fi"));
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
        assert!(script.contains("else echo 'mino: capsh not found"));
        assert!(script.contains("exit 1; fi"));
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
        assert!(script.contains("else echo 'mino: capsh not found"));
        assert!(script.contains("exit 1; fi"));
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

        assert!(script.contains("else echo 'mino: capsh not found"));
        assert!(script.contains("exit 1; fi"));
    }
}
