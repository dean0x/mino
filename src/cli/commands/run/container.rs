//! Container configuration building

use crate::cache::CacheMount;
use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::error::MinoResult;
use crate::network::NetworkMode;
use crate::orchestration::ContainerConfig;
use std::collections::HashMap;
use std::env;
use std::path::Path;

use super::ImageResolution;

/// Parameters for building a container configuration.
pub(super) struct ContainerBuildParams<'a> {
    pub args: &'a RunArgs,
    pub config: &'a Config,
    pub project_dir: &'a Path,
    pub resolution: &'a ImageResolution,
    pub env_vars: HashMap<String, String>,
    pub cache_mounts: &'a [CacheMount],
    pub cache_env: HashMap<String, String>,
    pub network_mode: &'a NetworkMode,
    pub home_mount: Option<String>,
}

/// Derive container workdir from project directory name.
/// Falls back to /workspace for system dir conflicts or if user overrode the config.
fn resolve_workdir(config_workdir: &str, project_dir: &Path) -> String {
    // User explicitly set a custom workdir — respect it
    if config_workdir != "/workspace" {
        return config_workdir.to_string();
    }

    let folder_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");

    // Block system directory names and mino-reserved paths to prevent overlay
    const BLOCKED: &[&str] = &[
        "bin",
        "dev",
        "etc",
        "home",
        "lib",
        "lib64",
        "opt",
        "proc",
        "root",
        "run",
        "sbin",
        "sys",
        "tmp",
        "usr",
        "var",
        "cache",
        "workspace",
        "ssh-agent", // SSH agent socket mount point
    ];

    if BLOCKED.contains(&folder_name) {
        return "/workspace".to_string();
    }

    format!("/{folder_name}")
}

/// Build the container configuration from resolved parameters.
pub(super) fn build_container_config(params: &ContainerBuildParams) -> MinoResult<ContainerConfig> {
    let image = params.resolution.image.clone();
    let workdir = resolve_workdir(&params.config.container.workdir, params.project_dir);

    let mut volumes = Vec::new();

    // Home volume mount (before other mounts for correct overlay order)
    if let Some(ref home) = params.home_mount {
        volumes.push(home.clone());
    }

    volumes.push(format!("{}:{}", params.project_dir.display(), workdir));

    volumes.extend(params.cache_mounts.iter().map(|m| m.volume_arg()));

    if !params.args.no_ssh_agent {
        if let Ok(sock) = env::var("SSH_AUTH_SOCK") {
            volumes.push(format!("{}:/ssh-agent", sock));
        }
    }

    volumes.extend(params.args.volume.iter().cloned());
    volumes.extend(params.config.container.volumes.iter().cloned());

    // Env precedence: config < layer < cache < credential < CLI -e
    let mut final_env = params.config.container.env.clone();
    final_env.extend(params.resolution.layer_env.clone());
    final_env.extend(params.cache_env.clone());
    final_env.extend(params.env_vars.clone());

    if !params.args.no_ssh_agent && env::var("SSH_AUTH_SOCK").is_ok() {
        final_env.insert("SSH_AUTH_SOCK".to_string(), "/ssh-agent".to_string());
    }

    let read_only = params.args.read_only || params.config.container.read_only;

    Ok(ContainerConfig {
        image,
        workdir,
        volumes,
        env: final_env,
        network: params.network_mode.to_podman_network().to_string(),
        interactive: !params.args.detach,
        tty: !params.args.detach,
        cap_drop: vec!["ALL".to_string()],
        cap_add: if params.network_mode.requires_cap_net_admin() {
            vec!["NET_ADMIN".to_string()]
        } else {
            vec![]
        },
        security_opt: vec!["no-new-privileges".to_string()],
        pids_limit: 4096,
        auto_remove: params.args.detach,
        read_only,
        tmpfs: if read_only {
            let mut mounts = vec!["/tmp".to_string(), "/run".to_string(), "/root".to_string()];
            // Only add /home/developer tmpfs if no home volume is mounted
            if params.home_mount.is_none() {
                mounts.push("/home/developer".to_string());
            }
            mounts
        } else {
            vec![]
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::RunArgs;
    use std::path::PathBuf;

    fn test_run_args() -> RunArgs {
        RunArgs {
            name: None,
            project: None,
            aws: false,
            gcp: false,
            azure: false,
            all_clouds: false,
            no_ssh_agent: true, // disable to avoid SSH_AUTH_SOCK dependency
            no_github: false,
            strict_credentials: false,
            image: None,
            layers: vec![],
            env: vec![],
            volume: vec![],
            detach: false,
            read_only: false,
            no_cache: false,
            no_home: false,
            cache_fresh: false,
            network: None,
            network_allow: vec![],
            network_preset: None,
            command: vec![],
        }
    }

    fn test_resolution() -> ImageResolution {
        ImageResolution {
            image: "fedora:43".to_string(),
            layer_env: HashMap::new(),
        }
    }

    fn build_with(args: &RunArgs, config: &Config) -> crate::orchestration::ContainerConfig {
        build_with_home(args, config, None)
    }

    fn build_with_home(
        args: &RunArgs,
        config: &Config,
        home_mount: Option<String>,
    ) -> crate::orchestration::ContainerConfig {
        let resolution = test_resolution();
        let project_dir = PathBuf::from("/tmp/project");
        let network_mode = NetworkMode::Bridge;
        let params = ContainerBuildParams {
            args,
            config,
            project_dir: &project_dir,
            resolution: &resolution,
            env_vars: HashMap::new(),
            cache_mounts: &[],
            cache_env: HashMap::new(),
            network_mode: &network_mode,
            home_mount,
        };
        build_container_config(&params).unwrap()
    }

    #[test]
    fn read_only_disabled_by_default() {
        let args = test_run_args();
        let config = Config::default();
        let result = build_with(&args, &config);
        assert!(!result.read_only);
        assert!(result.tmpfs.is_empty());
    }

    #[test]
    fn read_only_from_cli_flag() {
        let mut args = test_run_args();
        args.read_only = true;
        let config = Config::default();
        let result = build_with(&args, &config);
        assert!(result.read_only);
        // Without home_mount, /home/developer gets tmpfs
        assert!(result.tmpfs.contains(&"/tmp".to_string()));
        assert!(result.tmpfs.contains(&"/run".to_string()));
        assert!(result.tmpfs.contains(&"/root".to_string()));
        assert!(result.tmpfs.contains(&"/home/developer".to_string()));
    }

    #[test]
    fn read_only_from_config() {
        let args = test_run_args();
        let mut config = Config::default();
        config.container.read_only = true;
        let result = build_with(&args, &config);
        assert!(result.read_only);
        assert!(result.tmpfs.contains(&"/tmp".to_string()));
        assert!(result.tmpfs.contains(&"/run".to_string()));
        assert!(result.tmpfs.contains(&"/root".to_string()));
        assert!(result.tmpfs.contains(&"/home/developer".to_string()));
    }

    #[test]
    fn read_only_cli_or_config_either_enables() {
        let mut args = test_run_args();
        args.read_only = true;
        let mut config = Config::default();
        config.container.read_only = true;
        let result = build_with(&args, &config);
        assert!(result.read_only);
        assert!(!result.tmpfs.is_empty());
    }

    #[test]
    fn home_mount_appears_in_volumes() {
        let args = test_run_args();
        let config = Config::default();
        let result = build_with_home(
            &args,
            &config,
            Some("mino-home-abc123:/home/developer".to_string()),
        );
        assert!(result
            .volumes
            .contains(&"mino-home-abc123:/home/developer".to_string()));
    }

    #[test]
    fn read_only_with_home_excludes_developer_tmpfs() {
        let mut args = test_run_args();
        args.read_only = true;
        let config = Config::default();
        let result = build_with_home(
            &args,
            &config,
            Some("mino-home-abc123:/home/developer".to_string()),
        );
        assert!(result.read_only);
        assert!(result.tmpfs.contains(&"/tmp".to_string()));
        assert!(result.tmpfs.contains(&"/run".to_string()));
        assert!(result.tmpfs.contains(&"/root".to_string()));
        assert!(!result.tmpfs.contains(&"/home/developer".to_string()));
    }

    #[test]
    fn read_only_without_home_includes_developer_tmpfs() {
        let mut args = test_run_args();
        args.read_only = true;
        let config = Config::default();
        let result = build_with_home(&args, &config, None);
        assert!(result.read_only);
        assert!(result.tmpfs.contains(&"/home/developer".to_string()));
    }

    #[test]
    fn workdir_derived_from_project_dir() {
        let args = test_run_args();
        let config = Config::default();
        let result = build_with(&args, &config);
        // project_dir is /tmp/project → workdir should be /project
        assert_eq!(result.workdir, "/project");
        assert!(result.volumes.iter().any(|v| v.ends_with(":/project")));
    }

    #[test]
    fn workdir_blocked_name_falls_back() {
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/home/dev/bin")),
            "/workspace"
        );
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/home/dev/etc")),
            "/workspace"
        );
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/home/dev/tmp")),
            "/workspace"
        );
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/home/dev/cache")),
            "/workspace"
        );
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/home/dev/ssh-agent")),
            "/workspace"
        );
    }

    #[test]
    fn workdir_custom_config_preserved() {
        assert_eq!(
            resolve_workdir("/code", Path::new("/home/dev/my-project")),
            "/code"
        );
    }

    #[test]
    fn workdir_root_falls_back() {
        assert_eq!(resolve_workdir("/workspace", Path::new("/")), "/workspace");
    }

    #[test]
    fn workdir_normal_project_names() {
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/home/dev/my-app")),
            "/my-app"
        );
        assert_eq!(
            resolve_workdir("/workspace", Path::new("/Users/dean/Sandbox/minotaur")),
            "/minotaur"
        );
    }
}
