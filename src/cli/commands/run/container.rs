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
}

/// Build the container configuration from resolved parameters.
pub(super) fn build_container_config(params: &ContainerBuildParams) -> MinoResult<ContainerConfig> {
    let image = params.resolution.image.clone();

    let mut volumes = vec![format!(
        "{}:{}",
        params.project_dir.display(),
        params.config.container.workdir
    )];

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

    Ok(ContainerConfig {
        image,
        workdir: params.config.container.workdir.clone(),
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
    })
}
