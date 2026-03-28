//! Resource limits for sandboxed processes (POSIX setrlimit wrapper)
//!
//! Converts human-friendly config values (MB, count) to kernel-level resource
//! limits and generates prlimit command arguments for Linux enforcement.

use serde::{Deserialize, Serialize};

/// Bytes per megabyte for unit conversion
const BYTES_PER_MB: u64 = 1024 * 1024;

/// Resource limits to enforce on sandboxed process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Max virtual memory in bytes (0 = no limit)
    pub max_memory_bytes: u64,

    /// Max processes/threads
    pub max_processes: u32,

    /// Max CPU time in seconds (0 = no limit)
    pub max_cpu_seconds: u64,

    /// Max file size in bytes (0 = no limit)
    pub max_file_size_bytes: u64,
}

impl ResourceLimits {
    /// Create from SandboxConfig, converting MB to bytes
    pub fn from_config(config: &super::config::SandboxConfig) -> Self {
        Self {
            max_memory_bytes: config.max_memory_mb.saturating_mul(BYTES_PER_MB),
            max_processes: config.max_processes,
            max_cpu_seconds: config.max_cpu_seconds,
            max_file_size_bytes: config.max_file_size_mb.saturating_mul(BYTES_PER_MB),
        }
    }

    /// Generate prlimit command arguments for Linux
    ///
    /// Returns a list of arguments like `--as=4294967296` for each non-zero limit.
    /// Zero values are treated as "no limit" and produce no argument.
    pub fn to_prlimit_args(&self) -> Vec<String> {
        [
            (self.max_memory_bytes, "as"),
            (u64::from(self.max_processes), "nproc"),
            (self.max_cpu_seconds, "cpu"),
            (self.max_file_size_bytes, "fsize"),
        ]
        .into_iter()
        .filter(|(val, _)| *val > 0)
        .map(|(val, name)| format!("--{}={}", name, val))
        .collect()
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::from_config(&super::config::SandboxConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::config::SandboxConfig;

    #[test]
    fn mb_to_bytes_conversion() {
        let config = SandboxConfig {
            max_memory_mb: 4096,
            max_file_size_mb: 100,
            ..Default::default()
        };
        let limits = ResourceLimits::from_config(&config);
        assert_eq!(limits.max_memory_bytes, 4096 * 1024 * 1024);
        assert_eq!(limits.max_file_size_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn zero_means_no_limit() {
        let config = SandboxConfig {
            max_memory_mb: 0,
            max_cpu_seconds: 0,
            max_file_size_mb: 0,
            ..Default::default()
        };
        let limits = ResourceLimits::from_config(&config);
        assert_eq!(limits.max_memory_bytes, 0);
        assert_eq!(limits.max_cpu_seconds, 0);
        assert_eq!(limits.max_file_size_bytes, 0);
    }

    #[test]
    fn prlimit_args_with_all_limits() {
        let limits = ResourceLimits {
            max_memory_bytes: 4294967296, // 4 GB
            max_processes: 256,
            max_cpu_seconds: 3600,
            max_file_size_bytes: 104857600, // 100 MB
        };
        let args = limits.to_prlimit_args();
        assert_eq!(args.len(), 4);
        assert!(args.contains(&"--as=4294967296".to_string()));
        assert!(args.contains(&"--nproc=256".to_string()));
        assert!(args.contains(&"--cpu=3600".to_string()));
        assert!(args.contains(&"--fsize=104857600".to_string()));
    }

    #[test]
    fn prlimit_args_zero_values_omitted() {
        let limits = ResourceLimits {
            max_memory_bytes: 0,
            max_processes: 256,
            max_cpu_seconds: 0,
            max_file_size_bytes: 0,
        };
        let args = limits.to_prlimit_args();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "--nproc=256");
    }

    #[test]
    fn prlimit_args_all_zero_empty() {
        let limits = ResourceLimits {
            max_memory_bytes: 0,
            max_processes: 0,
            max_cpu_seconds: 0,
            max_file_size_bytes: 0,
        };
        let args = limits.to_prlimit_args();
        assert!(args.is_empty());
    }

    #[test]
    fn default_values() {
        let limits = ResourceLimits::default();
        // 4096 MB default from SandboxConfig
        assert_eq!(limits.max_memory_bytes, 4096 * 1024 * 1024);
        assert_eq!(limits.max_processes, 256);
        assert_eq!(limits.max_cpu_seconds, 0);
        assert_eq!(limits.max_file_size_bytes, 0);
    }

    #[test]
    fn saturating_mul_prevents_overflow() {
        let config = SandboxConfig {
            max_memory_mb: u64::MAX,
            ..Default::default()
        };
        let limits = ResourceLimits::from_config(&config);
        assert_eq!(limits.max_memory_bytes, u64::MAX);
    }
}
