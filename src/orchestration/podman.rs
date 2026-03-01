//! Podman shared types and helpers
//!
//! Contains data structures and shared argument-building logic
//! used by both `NativePodmanRuntime` and `OrbStackRuntime`.

use std::collections::HashMap;

/// Container configuration for running a new container
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Container image to use
    pub image: String,
    /// Working directory inside the container
    pub workdir: String,
    /// Volume mounts (host:container format)
    pub volumes: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Network mode
    pub network: String,
    /// Enable interactive mode
    pub interactive: bool,
    /// Allocate a TTY
    pub tty: bool,
    /// Linux capabilities to add (e.g., "NET_ADMIN")
    pub cap_add: Vec<String>,
    /// Linux capabilities to drop (e.g., "ALL")
    pub cap_drop: Vec<String>,
    /// Security options (e.g., "no-new-privileges")
    pub security_opt: Vec<String>,
    /// PID limit (0 = no limit)
    pub pids_limit: u32,
    /// Automatically remove container when it exits (--rm)
    pub auto_remove: bool,
}

impl ContainerConfig {
    /// Append Podman container arguments to a command-line argument vector.
    ///
    /// Pushes workdir, network, capabilities (drop before add), security options,
    /// pids-limit, volumes, env vars, image, and the user command.
    ///
    /// Used by both `NativePodmanRuntime` and `OrbStackRuntime`.
    pub fn push_args(&self, args: &mut Vec<String>, command: &[String]) {
        if self.auto_remove {
            args.push("--rm".to_string());
        }
        args.push("-w".to_string());
        args.push(self.workdir.clone());
        args.push("--network".to_string());
        args.push(self.network.clone());

        // cap-drop BEFORE cap-add: Podman processes them in order
        for cap in &self.cap_drop {
            args.push("--cap-drop".to_string());
            args.push(cap.clone());
        }
        for cap in &self.cap_add {
            args.push("--cap-add".to_string());
            args.push(cap.clone());
        }
        for opt in &self.security_opt {
            args.push("--security-opt".to_string());
            args.push(opt.clone());
        }
        if self.pids_limit > 0 {
            args.push("--pids-limit".to_string());
            args.push(self.pids_limit.to_string());
        }

        for v in &self.volumes {
            args.push("-v".to_string());
            args.push(v.clone());
        }
        for (k, v) in &self.env {
            args.push("-e".to_string());
            args.push(format!("{}={}", k, v));
        }

        args.push(self.image.clone());
        args.extend(command.iter().cloned());
    }
}

/// Env var keys whose values must never appear in logs.
const SENSITIVE_ENV_KEYS: &[&str] = &[
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "CLOUDSDK_AUTH_ACCESS_TOKEN",
    "AZURE_ACCESS_TOKEN",
];

/// Return a copy of `args` with sensitive `-e KEY=VALUE` values replaced by `***`.
///
/// Generic over element type so it works with both `&[String]` and `&[&str]`.
pub(crate) fn redact_args<S: AsRef<str>>(args: &[S]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        let s = arg.as_ref();
        if redact_next {
            if let Some((key, _)) = s.split_once('=') {
                if SENSITIVE_ENV_KEYS.contains(&key) {
                    out.push(format!("{key}=***"));
                } else {
                    out.push(s.to_owned());
                }
            } else {
                out.push(s.to_owned());
            }
            redact_next = false;
        } else {
            out.push(s.to_owned());
            redact_next = s == "-e";
        }
    }
    out
}

/// Information about a running container
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    /// Container ID
    pub id: String,
    /// Container name
    pub name: String,
    /// Container status (e.g., "Up 2 hours")
    pub status: String,
    /// Container image
    pub image: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ContainerConfig {
        ContainerConfig {
            image: "fedora:43".to_string(),
            workdir: "/workspace".to_string(),
            volumes: vec![],
            env: HashMap::new(),
            network: "bridge".to_string(),
            interactive: true,
            tty: true,
            cap_add: vec![],
            cap_drop: vec!["ALL".to_string()],
            security_opt: vec!["no-new-privileges".to_string()],
            pids_limit: 4096,
            auto_remove: false,
        }
    }

    #[test]
    fn container_config_fields() {
        let config = test_config();
        assert_eq!(config.image, "fedora:43");
        assert_eq!(config.cap_drop, vec!["ALL"]);
        assert_eq!(config.security_opt, vec!["no-new-privileges"]);
        assert_eq!(config.pids_limit, 4096);
    }

    #[test]
    fn push_args_cap_drop_before_cap_add() {
        let mut config = test_config();
        config.cap_add = vec!["NET_ADMIN".to_string()];

        let mut args = Vec::new();
        config.push_args(&mut args, &[]);

        let drop_pos = args.iter().position(|a| a == "--cap-drop").unwrap();
        let add_pos = args.iter().position(|a| a == "--cap-add").unwrap();
        assert!(drop_pos < add_pos, "--cap-drop must come before --cap-add");

        assert!(args.contains(&"--security-opt".to_string()));
        assert!(args.contains(&"no-new-privileges".to_string()));
        assert!(args.contains(&"--pids-limit".to_string()));
        assert!(args.contains(&"4096".to_string()));
    }

    #[test]
    fn push_args_auto_remove() {
        let mut config = test_config();
        config.auto_remove = true;

        let mut args = Vec::new();
        config.push_args(&mut args, &["echo".to_string()]);
        assert_eq!(args[0], "--rm", "--rm must be first arg when auto_remove");

        // Verify --rm is absent when auto_remove is false
        config.auto_remove = false;
        let mut args = Vec::new();
        config.push_args(&mut args, &[]);
        assert!(!args.contains(&"--rm".to_string()));
    }

    #[test]
    fn redact_args_masks_sensitive_keys() {
        let args: Vec<String> = vec![
            "run",
            "-d",
            "-e",
            "AWS_SECRET_ACCESS_KEY=hunter2",
            "-e",
            "GITHUB_TOKEN=ghp_abc123",
            "-e",
            "PATH=/usr/bin",
            "fedora:43",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let redacted = redact_args(&args);
        assert_eq!(redacted[3], "AWS_SECRET_ACCESS_KEY=***");
        assert_eq!(redacted[5], "GITHUB_TOKEN=***");
        assert_eq!(redacted[7], "PATH=/usr/bin");
    }

    #[test]
    fn redact_args_preserves_non_sensitive() {
        let args: Vec<String> = vec![
            "run",
            "-e",
            "HOME=/home/dev",
            "-e",
            "LANG=en_US.UTF-8",
            "-w",
            "/workspace",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let redacted = redact_args(&args);
        assert_eq!(redacted, args);
    }

    #[test]
    fn redact_args_handles_no_env() {
        let args: Vec<String> = vec!["run", "-d", "-w", "/workspace", "fedora:43"]
            .into_iter()
            .map(String::from)
            .collect();

        let redacted = redact_args(&args);
        assert_eq!(redacted, args);
    }

    #[test]
    fn redact_args_works_with_str_slices() {
        let args: &[&str] = &[
            "run",
            "-d",
            "-e",
            "AWS_SESSION_TOKEN=secret123",
            "-e",
            "HOME=/home/dev",
        ];

        let redacted = redact_args(args);
        assert_eq!(redacted[3], "AWS_SESSION_TOKEN=***");
        assert_eq!(redacted[5], "HOME=/home/dev");
    }

    #[test]
    fn redact_args_trailing_dash_e_no_panic() {
        let args: Vec<String> = vec!["run", "-e"].into_iter().map(String::from).collect();

        let redacted = redact_args(&args);
        assert_eq!(redacted, args);
    }

    #[test]
    fn push_args_no_pids_limit_when_zero() {
        let mut config = test_config();
        config.pids_limit = 0;
        config.cap_drop = vec![];
        config.security_opt = vec![];

        let mut args = Vec::new();
        config.push_args(&mut args, &[]);
        assert!(!args.contains(&"--pids-limit".to_string()));
    }
}
