//! Cache setup and finalization

use crate::cache::{
    detect_lockfiles, format_bytes, gb_to_bytes, resolve_state, CacheMount, CacheSidecar,
    CacheSizeStatus, CacheState, CacheVolume, LockfileInfo,
};
use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::orchestration::ContainerRuntime;
use console::style;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, warn};

use super::CacheSession;

/// Setup cache volumes and environment variables
pub(super) async fn setup_caches(
    runtime: &dyn ContainerRuntime,
    args: &RunArgs,
    config: &Config,
    project_dir: &Path,
) -> MinoResult<(Vec<CacheMount>, HashMap<String, String>, CacheSession)> {
    let mut cache_session = CacheSession::default();
    let mut cache_mounts = Vec::new();
    let mut cache_env = HashMap::new();

    if args.no_cache || !config.cache.enabled {
        debug!("Caching disabled");
        return Ok((cache_mounts, cache_env, cache_session));
    }

    let lockfiles = {
        let dir = project_dir.to_path_buf();
        tokio::task::spawn_blocking(move || detect_lockfiles(&dir))
            .await
            .map_err(|e| MinoError::Internal(format!("lockfile detection task failed: {e}")))?
    }?;
    if lockfiles.is_empty() {
        debug!("No lockfiles detected, skipping cache setup");
        return Ok((cache_mounts, cache_env, cache_session));
    }

    debug!("Detected {} lockfile(s)", lockfiles.len());

    for info in &lockfiles {
        let (mount, should_finalize) =
            setup_cache_for_lockfile(runtime, info, args.cache_fresh).await?;

        for (key, value) in info.ecosystem.cache_env_vars() {
            cache_env.insert(key.to_string(), value.to_string());
        }

        if should_finalize {
            cache_session
                .volumes_to_finalize
                .push(mount.volume_name.clone());
        }

        cache_mounts.push(mount);
    }

    cache_env.insert("XDG_CACHE_HOME".to_string(), "/cache/xdg".to_string());

    Ok((cache_mounts, cache_env, cache_session))
}

/// Setup cache for a single lockfile, returns (mount, should_finalize)
async fn setup_cache_for_lockfile(
    runtime: &dyn ContainerRuntime,
    info: &LockfileInfo,
    force_fresh: bool,
) -> MinoResult<(CacheMount, bool)> {
    let volume_name = info.volume_name();

    if force_fresh {
        CacheSidecar::delete(&volume_name).await.ok();
    }

    let existing = if force_fresh {
        None
    } else {
        runtime.volume_inspect(&volume_name).await?
    };

    let (state, should_finalize) = match existing {
        Some(vol_info) => {
            let label_state = CacheVolume::from_labels(&vol_info.name, &vol_info.labels)
                .map(|c| c.state)
                .unwrap_or(CacheState::Building);

            // Use sidecar as authoritative state source, fall back to labels
            let resolved = resolve_state(&volume_name, label_state).await;

            match resolved {
                CacheState::Complete => {
                    debug!(
                        "Cache hit for {} ({}), mounting read-only",
                        info.ecosystem,
                        &info.hash[..8]
                    );
                    (CacheState::Complete, false)
                }
                CacheState::Building | CacheState::Miss => {
                    debug!(
                        "Resuming incomplete cache for {} ({})",
                        info.ecosystem,
                        &info.hash[..8]
                    );
                    // Backfill sidecar for existing volumes that lack one (backward compat)
                    if CacheSidecar::load(&volume_name)
                        .await
                        .ok()
                        .flatten()
                        .is_none()
                    {
                        let mut sidecar = CacheSidecar::new(
                            volume_name.clone(),
                            info.ecosystem,
                            info.hash.clone(),
                            CacheState::Building,
                        );
                        if let Err(e) = sidecar.save().await {
                            warn!("Failed to backfill sidecar for {}: {}", volume_name, e);
                        }
                    }
                    (CacheState::Building, true)
                }
            }
        }
        None => {
            debug!(
                "Cache miss for {} ({}), creating volume",
                info.ecosystem,
                &info.hash[..8]
            );

            let cache = CacheVolume::from_lockfile(info, CacheState::Building);
            runtime.volume_create(&volume_name, &cache.labels()).await?;

            let mut sidecar = CacheSidecar::new(
                volume_name.clone(),
                info.ecosystem,
                info.hash.clone(),
                CacheState::Building,
            );
            if let Err(e) = sidecar.save().await {
                warn!("Failed to create sidecar for {}: {}", volume_name, e);
            }

            // Re-inspect: another process may have created it first with different state
            let resolved = match runtime.volume_inspect(&volume_name).await? {
                Some(vol_info) => {
                    let label_state = CacheVolume::from_labels(&vol_info.name, &vol_info.labels)
                        .map(|c| c.state)
                        .unwrap_or(CacheState::Building);
                    resolve_state(&volume_name, label_state).await
                }
                None => CacheState::Building,
            };

            if resolved == CacheState::Complete {
                (CacheState::Complete, false)
            } else {
                (CacheState::Building, true)
            }
        }
    };

    let mount = CacheMount {
        volume_name,
        container_path: "/cache".to_string(),
        readonly: state.is_readonly(),
        ecosystem: info.ecosystem,
    };

    Ok((mount, should_finalize))
}

/// Finalize cache volumes by marking their sidecar state as complete.
///
/// This is the fix for the original bug: Podman volume labels are immutable
/// after creation, so state transitions are now tracked via sidecar JSON files.
/// Finalization is best-effort -- failures are logged but do not fail the session.
pub(super) async fn finalize_caches(cache_session: &CacheSession) {
    for volume_name in &cache_session.volumes_to_finalize {
        debug!("Finalizing cache: {}", volume_name);

        match CacheSidecar::load(volume_name).await {
            Ok(Some(mut sidecar)) => {
                if let Err(e) = sidecar.mark_complete().await {
                    warn!("Failed to finalize cache sidecar {}: {}", volume_name, e);
                } else {
                    debug!("Cache {} finalized (complete via sidecar)", volume_name);
                }
            }
            Ok(None) => {
                warn!(
                    "No sidecar found for cache {}, skipping finalization",
                    volume_name
                );
            }
            Err(e) => {
                warn!("Failed to load cache sidecar {}: {}", volume_name, e);
            }
        }
    }
}

/// Check cache size and print warning if approaching or exceeding limit
pub(super) async fn check_cache_size_warning(runtime: &dyn ContainerRuntime, config: &Config) {
    let sizes = match runtime.volume_disk_usage("mino-cache-").await {
        Ok(s) => s,
        Err(_) => return, // Silently skip if we can't get sizes
    };

    let total_size: u64 = sizes.values().sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);

    if limit_bytes == 0 {
        return;
    }

    let status = CacheSizeStatus::from_usage(total_size, limit_bytes);
    let percent = CacheSizeStatus::percentage(total_size, limit_bytes);

    match status {
        CacheSizeStatus::Ok => {}
        CacheSizeStatus::Warning => {
            eprintln!(
                "{} Cache usage at {:.0}% ({} / {}). Consider running: mino cache gc",
                style("!").yellow(),
                percent,
                format_bytes(total_size),
                format_bytes(limit_bytes)
            );
        }
        CacheSizeStatus::Exceeded => {
            eprintln!(
                "{} Cache limit exceeded! {:.0}% ({} / {}). Run: mino cache gc",
                style("!").red().bold(),
                percent,
                format_bytes(total_size),
                format_bytes(limit_bytes)
            );
        }
    }
}
