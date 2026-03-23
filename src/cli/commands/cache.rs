//! Cache command - manage dependency caches

use crate::cache::{
    detect_lockfiles, format_bytes, gb_to_bytes, resolve_state, CacheSidecar, CacheSizeStatus,
    CacheState, CacheVolume,
};
use crate::cli::args::{CacheAction, CacheArgs, OutputFormat};
use crate::cli::commands::run::image::LAYER_BASE_IMAGE;
use crate::config::Config;
use crate::error::{MinoError, MinoResult};
use crate::home::HomeVolume;
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::ui::{self, UiContext};
use chrono::Utc;
use console::{pad_str, style, Alignment};
use std::env;
use std::path::PathBuf;
use tracing::debug;

/// Execute the cache command
pub async fn execute(args: CacheArgs, config: &Config) -> MinoResult<()> {
    let runtime = create_runtime(config)?;

    match args.action {
        CacheAction::List { format } => list_caches(&*runtime, format, config).await,
        CacheAction::Info { project } => show_project_info(&*runtime, project, config).await,
        CacheAction::Gc { days, dry_run } => gc_caches(&*runtime, config, days, dry_run).await,
        CacheAction::Clear {
            all,
            volumes,
            images,
            home,
            yes,
        } => clear_artifacts(&*runtime, all || volumes, all || images, all || home, yes).await,
    }
}

/// List all cache volumes with sizes
async fn list_caches(
    runtime: &dyn ContainerRuntime,
    format: OutputFormat,
    config: &Config,
) -> MinoResult<()> {
    let volumes = runtime.volume_list("mino-cache-").await?;
    let home_volumes = runtime.volume_list("mino-home-").await?;

    if volumes.is_empty() && home_volumes.is_empty() {
        match format {
            OutputFormat::Json => println!("{{\"caches\":[],\"home_volumes\":[]}}"),
            OutputFormat::Plain => {}
            OutputFormat::Table => println!("No cache or home volumes found."),
        }
        return Ok(());
    }

    // Get disk usage for all cache volumes
    let sizes = if !volumes.is_empty() {
        runtime.volume_disk_usage("mino-cache-").await?
    } else {
        std::collections::HashMap::new()
    };

    // Parse into CacheVolume structs with sizes, resolving state via sidecar
    let mut caches: Vec<(CacheVolume, u64)> = Vec::new();
    for v in &volumes {
        if let Some(mut cache) = CacheVolume::from_labels(&v.name, &v.labels) {
            cache.state = resolve_state(&cache.name, cache.state).await;
            let size = *sizes.get(&cache.name).unwrap_or(&0);
            caches.push((cache, size));
        }
    }

    // Parse home volumes
    let home_vols: Vec<HomeVolume> = home_volumes
        .iter()
        .filter_map(|v| HomeVolume::from_labels(&v.name, &v.labels))
        .collect();

    // Calculate total size
    let total_size: u64 = caches.iter().map(|(_, s)| s).sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);

    match format {
        OutputFormat::Table => {
            print_cache_table(&caches, total_size, limit_bytes);
            if !home_vols.is_empty() {
                print_home_table(&home_vols);
            }
        }
        OutputFormat::Json => print_cache_json(&caches, &home_vols, total_size, limit_bytes)?,
        OutputFormat::Plain => {
            print_cache_plain(&caches);
            for hv in &home_vols {
                println!("{}", hv.name);
            }
        }
    }

    Ok(())
}

fn print_cache_table(caches: &[(CacheVolume, u64)], total_size: u64, limit_bytes: u64) {
    const W_VOLUME: usize = 40;
    const W_ECO: usize = 10;
    const W_STATE: usize = 10;
    const W_SIZE: usize = 10;
    const W_CREATED: usize = 16;

    let ctx = UiContext::detect();

    ui::intro(&ctx, "Cache Volumes");

    println!(
        "{} {} {} {} {}",
        pad_str("VOLUME", W_VOLUME, Alignment::Left, None),
        pad_str("ECOSYSTEM", W_ECO, Alignment::Left, None),
        pad_str("STATE", W_STATE, Alignment::Left, None),
        pad_str("SIZE", W_SIZE, Alignment::Left, None),
        pad_str("CREATED", W_CREATED, Alignment::Left, None),
    );
    println!(
        "{}",
        "-".repeat(W_VOLUME + 1 + W_ECO + 1 + W_STATE + 1 + W_SIZE + 1 + W_CREATED)
    );

    for (cache, size) in caches {
        let state_display = match cache.state {
            CacheState::Complete => style("complete").green().to_string(),
            CacheState::Building => style("building").yellow().to_string(),
            CacheState::Miss => style("miss").dim().to_string(),
        };

        let size_display = if *size > 0 {
            format_bytes(*size)
        } else {
            "-".to_string()
        };

        let created = cache.created_at.format("%Y-%m-%d %H:%M").to_string();

        println!(
            "{} {} {} {} {}",
            pad_str(&cache.name, W_VOLUME, Alignment::Left, None),
            pad_str(&cache.ecosystem.to_string(), W_ECO, Alignment::Left, None),
            pad_str(&state_display, W_STATE, Alignment::Left, None),
            pad_str(&size_display, W_SIZE, Alignment::Left, None),
            pad_str(&created, W_CREATED, Alignment::Left, None),
        );
    }

    println!();

    // Show total and limit
    let status = CacheSizeStatus::from_usage(total_size, limit_bytes);
    let percent = CacheSizeStatus::percentage(total_size, limit_bytes);

    let total_display = format!(
        "Total: {} / {} ({:.0}%)",
        format_bytes(total_size),
        format_bytes(limit_bytes),
        percent
    );

    match status {
        CacheSizeStatus::Ok => println!("{}", total_display),
        CacheSizeStatus::Warning => println!(
            "{} {} - consider running: mino cache gc",
            style("!").yellow(),
            total_display
        ),
        CacheSizeStatus::Exceeded => println!(
            "{} {} - run: mino cache gc",
            style("!").red().bold(),
            total_display
        ),
    }

    println!("{} cache(s)", caches.len());
}

fn print_home_table(home_vols: &[HomeVolume]) {
    const W_VOLUME: usize = 40;
    const W_PROJECT: usize = 40;
    const W_CREATED: usize = 16;

    let ctx = UiContext::detect();

    ui::intro(&ctx, "Home Volumes");

    println!(
        "{} {} {}",
        pad_str("VOLUME", W_VOLUME, Alignment::Left, None),
        pad_str("PROJECT", W_PROJECT, Alignment::Left, None),
        pad_str("CREATED", W_CREATED, Alignment::Left, None),
    );
    println!("{}", "-".repeat(W_VOLUME + 1 + W_PROJECT + 1 + W_CREATED));

    for hv in home_vols {
        let created = hv.created_at.format("%Y-%m-%d %H:%M").to_string();
        println!(
            "{} {} {}",
            pad_str(&hv.name, W_VOLUME, Alignment::Left, None),
            pad_str(&hv.project_path, W_PROJECT, Alignment::Left, Some("...")),
            pad_str(&created, W_CREATED, Alignment::Left, None),
        );
    }

    println!();
    println!("{} home volume(s)", home_vols.len());
}

fn print_cache_json(
    caches: &[(CacheVolume, u64)],
    home_vols: &[HomeVolume],
    total_size: u64,
    limit_bytes: u64,
) -> MinoResult<()> {
    #[derive(serde::Serialize)]
    struct CacheJson {
        name: String,
        ecosystem: String,
        hash: String,
        state: String,
        size_bytes: u64,
        created_at: String,
    }

    #[derive(serde::Serialize)]
    struct HomeJson {
        name: String,
        project_path: String,
        created_at: String,
    }

    #[derive(serde::Serialize)]
    struct Output {
        caches: Vec<CacheJson>,
        home_volumes: Vec<HomeJson>,
        total_size_bytes: u64,
        limit_bytes: u64,
        usage_percent: f64,
    }

    let json_caches: Vec<CacheJson> = caches
        .iter()
        .map(|(c, size)| CacheJson {
            name: c.name.clone(),
            ecosystem: c.ecosystem.to_string(),
            hash: c.hash.clone(),
            state: c.state.to_string(),
            size_bytes: *size,
            created_at: c.created_at.to_rfc3339(),
        })
        .collect();

    let json_home: Vec<HomeJson> = home_vols
        .iter()
        .map(|hv| HomeJson {
            name: hv.name.clone(),
            project_path: hv.project_path.clone(),
            created_at: hv.created_at.to_rfc3339(),
        })
        .collect();

    let output = Output {
        caches: json_caches,
        home_volumes: json_home,
        total_size_bytes: total_size,
        limit_bytes,
        usage_percent: CacheSizeStatus::percentage(total_size, limit_bytes),
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_cache_plain(caches: &[(CacheVolume, u64)]) {
    for (cache, _) in caches {
        println!("{}", cache.name);
    }
}

/// Show cache info for a specific project
async fn show_project_info(
    runtime: &dyn ContainerRuntime,
    project: Option<PathBuf>,
    config: &Config,
) -> MinoResult<()> {
    let ctx = UiContext::detect();

    let project_dir = match project {
        Some(p) => p.canonicalize().unwrap_or(p),
        None => env::current_dir().map_err(|e| MinoError::io("getting current directory", e))?,
    };

    ui::intro(&ctx, "Project Cache Info");
    ui::key_value(&ctx, "Project", &project_dir.display().to_string());

    // Detect lockfiles
    let lockfiles = {
        let dir = project_dir.clone();
        tokio::task::spawn_blocking(move || detect_lockfiles(&dir))
            .await
            .map_err(|e| MinoError::Internal(format!("lockfile detection task failed: {e}")))?
    }?;

    if lockfiles.is_empty() {
        ui::step_info(&ctx, "No lockfiles detected in this project.");
        return Ok(());
    }

    ui::section(&ctx, "Detected lockfiles");
    for info in &lockfiles {
        ui::step_info(
            &ctx,
            &format!(
                "{} (hash: {})",
                info.path.file_name().unwrap_or_default().to_string_lossy(),
                &info.hash
            ),
        );
    }

    // Get disk usage
    let sizes = runtime.volume_disk_usage("mino-cache-").await?;

    // Check cache states
    ui::section(&ctx, "Cache status");
    let mut project_total: u64 = 0;

    for info in &lockfiles {
        let volume_name = info.volume_name();
        let volume_info = runtime.volume_inspect(&volume_name).await?;
        let size = sizes.get(&volume_name).copied().unwrap_or(0);
        project_total += size;

        let size_suffix = if size > 0 {
            format!(" ({})", format_bytes(size))
        } else {
            String::new()
        };

        let ecosystem = info.ecosystem.to_string();

        match volume_info {
            Some(v) => {
                let label_state = CacheVolume::from_labels(&v.name, &v.labels)
                    .map(|c| c.state)
                    .unwrap_or(CacheState::Building);
                let state = resolve_state(&volume_name, label_state).await;

                match state {
                    CacheState::Complete => {
                        ui::step_ok_detail(
                            &ctx,
                            &ecosystem,
                            &format!("complete (ro){}", size_suffix),
                        );
                    }
                    CacheState::Building | CacheState::Miss => {
                        ui::step_warn_hint(
                            &ctx,
                            &ecosystem,
                            &format!("building (rw){}", size_suffix),
                        );
                    }
                }
            }
            None => {
                ui::step_info(&ctx, &format!("{}: miss (will create)", ecosystem));
            }
        }
    }

    // Show total cache usage (reuse sizes from earlier query)
    let total_size: u64 = sizes.values().sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);
    let percent = CacheSizeStatus::percentage(total_size, limit_bytes);

    println!();
    ui::key_value(
        &ctx,
        "Project cache size",
        &if project_total > 0 {
            format_bytes(project_total)
        } else {
            "0 B".to_string()
        },
    );
    ui::key_value(
        &ctx,
        "Total cache usage",
        &format!(
            "{} / {} ({:.0}%)",
            format_bytes(total_size),
            format_bytes(limit_bytes),
            percent
        ),
    );

    Ok(())
}

/// Garbage collect old and orphaned caches
async fn gc_caches(
    runtime: &dyn ContainerRuntime,
    config: &Config,
    days_override: Option<u32>,
    dry_run: bool,
) -> MinoResult<()> {
    let ctx = UiContext::detect();
    let gc_days = days_override.unwrap_or(config.cache.gc_days);

    // Get current cache size
    let sizes = runtime.volume_disk_usage("mino-cache-").await?;
    let total_size: u64 = sizes.values().sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);

    ui::intro(&ctx, "Cache Garbage Collection");
    ui::key_value(
        &ctx,
        "Current usage",
        &format!(
            "{} / {} ({:.0}%)",
            format_bytes(total_size),
            format_bytes(limit_bytes),
            CacheSizeStatus::percentage(total_size, limit_bytes)
        ),
    );

    let volumes = runtime.volume_list("mino-cache-").await?;
    let caches: Vec<CacheVolume> = volumes
        .iter()
        .filter_map(|v| CacheVolume::from_labels(&v.name, &v.labels))
        .collect();

    // Find caches to remove (age-based)
    let mut to_remove: Vec<(&CacheVolume, u64)> = Vec::new();

    if gc_days > 0 {
        for cache in &caches {
            if cache.is_older_than_days(gc_days) {
                let size = sizes.get(&cache.name).copied().unwrap_or(0);
                to_remove.push((cache, size));
            }
        }
    }

    if to_remove.is_empty() {
        if gc_days > 0 {
            ui::step_ok(&ctx, &format!("No caches older than {} days.", gc_days));
        } else {
            ui::step_info(&ctx, "Cache GC by age is disabled (gc_days = 0).");
        }
        return Ok(());
    }

    let bytes_to_free: u64 = to_remove.iter().map(|(_, s)| s).sum();

    ui::section(
        &ctx,
        &format!(
            "Found {} cache(s) to remove ({})",
            to_remove.len(),
            format_bytes(bytes_to_free)
        ),
    );

    for (cache, size) in &to_remove {
        let age_days = (Utc::now() - cache.created_at).num_days();
        let size_str = if *size > 0 {
            format!(" ({})", format_bytes(*size))
        } else {
            String::new()
        };
        ui::step_warn(
            &ctx,
            &format!("{} - {} days old{}", cache.name, age_days, size_str),
        );
    }

    // Check home volumes for deleted projects
    let home_volumes = runtime.volume_list("mino-home-").await?;
    let mut home_to_remove: Vec<HomeVolume> = Vec::new();

    for v in &home_volumes {
        if let Some(hv) = HomeVolume::from_labels(&v.name, &v.labels) {
            let project_path = std::path::Path::new(&hv.project_path);
            if !project_path.exists() {
                home_to_remove.push(hv);
            }
        }
    }

    if !home_to_remove.is_empty() {
        ui::section(
            &ctx,
            &format!(
                "Found {} orphaned home volume(s) (project deleted)",
                home_to_remove.len()
            ),
        );
        for hv in &home_to_remove {
            ui::step_warn(&ctx, &format!("{} ({})", hv.name, hv.project_path));
        }
    }

    if dry_run {
        println!();
        ui::note(&ctx, "Dry run", "No caches removed.");
        return Ok(());
    }

    if to_remove.is_empty() && home_to_remove.is_empty() {
        return Ok(());
    }

    println!();
    let mut spinner = ui::TaskSpinner::new(&ctx);
    spinner.start("Removing caches...");

    let mut removed = 0;
    for (cache, _) in to_remove {
        debug!("Removing cache: {}", cache.name);
        runtime.volume_remove(&cache.name).await?;
        CacheSidecar::delete(&cache.name).await.ok();
        removed += 1;
    }

    let mut home_removed = 0;
    for hv in home_to_remove {
        debug!("Removing orphaned home volume: {}", hv.name);
        runtime.volume_remove(&hv.name).await?;
        home_removed += 1;
    }

    let mut summary_parts = Vec::new();
    if removed > 0 {
        summary_parts.push(format!(
            "{} cache(s), freed {}",
            removed,
            format_bytes(bytes_to_free)
        ));
    }
    if home_removed > 0 {
        summary_parts.push(format!("{} orphaned home volume(s)", home_removed));
    }
    spinner.stop(&format!("Removed {}", summary_parts.join(" + ")));

    Ok(())
}

/// Clear cache artifacts (volumes, images, home volumes, or all)
async fn clear_artifacts(
    runtime: &dyn ContainerRuntime,
    clear_volumes: bool,
    clear_images: bool,
    clear_home: bool,
    skip_confirm: bool,
) -> MinoResult<()> {
    let ctx = UiContext::detect();

    // Gather what will be deleted
    let volumes = if clear_volumes {
        runtime.volume_list("mino-cache-").await?
    } else {
        vec![]
    };

    let sizes = if !volumes.is_empty() {
        runtime.volume_disk_usage("mino-cache-").await?
    } else {
        std::collections::HashMap::new()
    };
    let total_volume_size: u64 = volumes
        .iter()
        .map(|v| sizes.get(&v.name).copied().unwrap_or(0))
        .sum();

    let images = if clear_images {
        runtime.image_list_prefixed("mino-composed-").await?
    } else {
        vec![]
    };

    let home_volumes = if clear_home {
        runtime.volume_list("mino-home-").await?
    } else {
        vec![]
    };

    if volumes.is_empty() && images.is_empty() && home_volumes.is_empty() && !clear_images {
        ui::intro(&ctx, "Cache Clear");
        ui::step_info(&ctx, "Nothing to clear.");
        return Ok(());
    }

    // Show summary
    ui::intro(&ctx, "Cache Clear");

    if !volumes.is_empty() {
        ui::step_warn(
            &ctx,
            &format!(
                "This will remove {} cache volume(s) ({})",
                volumes.len(),
                format_bytes(total_volume_size)
            ),
        );
        for vol in &volumes {
            let size = sizes.get(&vol.name).copied().unwrap_or(0);
            let size_str = if size > 0 {
                format!(" ({})", format_bytes(size))
            } else {
                String::new()
            };
            ui::remark(&ctx, &format!("{}{}", vol.name, size_str));
        }
    }

    if !images.is_empty() || clear_images {
        let count = images.len() + if clear_images { 1 } else { 0 };
        ui::step_warn(&ctx, &format!("This will remove up to {} image(s)", count));
        for img in &images {
            ui::remark(&ctx, img);
        }
        if clear_images {
            ui::remark(&ctx, LAYER_BASE_IMAGE);
        }
    }

    if !home_volumes.is_empty() {
        ui::step_warn(
            &ctx,
            &format!("This will remove {} home volume(s)", home_volumes.len()),
        );
        for vol in &home_volumes {
            let project = vol
                .labels
                .get("io.mino.home.project")
                .cloned()
                .unwrap_or_default();
            ui::remark(&ctx, &format!("{} ({})", vol.name, project));
        }
    }

    // Single confirmation
    if !skip_confirm {
        let confirmed = ui::confirm(&ctx, "Are you sure you want to proceed?", false).await?;
        if !confirmed {
            ui::outro_warn(&ctx, "Aborted.");
            return Ok(());
        }
    }

    let mut spinner = ui::TaskSpinner::new(&ctx);
    spinner.start("Clearing...");

    // Remove cache volumes and their sidecar files
    let vol_count = volumes.len();
    for vol in volumes {
        runtime.volume_remove(&vol.name).await?;
        CacheSidecar::delete(&vol.name).await.ok();
    }

    // Remove images (prune containers first so rmi doesn't fail)
    let img_count = images.len();
    if !images.is_empty() || clear_images {
        runtime.container_prune().await?;
        for img in &images {
            runtime.image_remove(img).await?;
        }
        // Also remove base image so it's re-pulled fresh on next run
        if clear_images {
            let _ = runtime.image_remove(LAYER_BASE_IMAGE).await;
        }
    }

    // Remove home volumes
    let home_count = home_volumes.len();
    for vol in home_volumes {
        runtime.volume_remove(&vol.name).await?;
    }

    // Summary
    let mut parts = Vec::new();
    if vol_count > 0 {
        parts.push(format!(
            "{} cache volume(s) ({})",
            vol_count,
            format_bytes(total_volume_size)
        ));
    }
    if img_count > 0 || clear_images {
        parts.push(format!(
            "{} image(s)",
            img_count + if clear_images { 1 } else { 0 }
        ));
    }
    if home_count > 0 {
        parts.push(format!("{} home volume(s)", home_count));
    }
    if parts.is_empty() {
        spinner.stop("Nothing to clear");
    } else {
        spinner.stop(&format!("Cleared {}", parts.join(" + ")));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::{MockResponse, MockRuntime};
    use crate::orchestration::VolumeInfo;
    use std::collections::HashMap;

    #[test]
    fn cache_state_display() {
        assert_eq!(CacheState::Complete.to_string(), "complete");
        assert_eq!(CacheState::Building.to_string(), "building");
        assert_eq!(CacheState::Miss.to_string(), "miss");
    }

    #[test]
    fn format_bytes_display() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    fn mino_cache_volume(name: &str) -> VolumeInfo {
        let labels = HashMap::from([
            ("io.mino.cache".to_string(), "true".to_string()),
            ("io.mino.cache.ecosystem".to_string(), "npm".to_string()),
            ("io.mino.cache.hash".to_string(), "abcdef123456".to_string()),
            ("io.mino.cache.state".to_string(), "complete".to_string()),
        ]);
        VolumeInfo {
            name: name.to_string(),
            labels,
            mountpoint: None,
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            size_bytes: None,
        }
    }

    #[tokio::test]
    async fn list_empty_volumes() {
        let mock = MockRuntime::new();
        let config = Config::default();

        list_caches(&mock, OutputFormat::Plain, &config)
            .await
            .unwrap();
        // Called twice: once for mino-cache-, once for mino-home-
        mock.assert_called("volume_list", 2);
    }

    #[tokio::test]
    async fn clear_volumes_removes_all() {
        let volumes = vec![
            mino_cache_volume("mino-cache-npm-abc123"),
            mino_cache_volume("mino-cache-cargo-def456"),
        ];
        let sizes = HashMap::from([
            ("mino-cache-npm-abc123".to_string(), 1024u64),
            ("mino-cache-cargo-def456".to_string(), 2048u64),
        ]);

        let mock = MockRuntime::new()
            .on("volume_list", Ok(MockResponse::VolumeInfoVec(volumes)))
            .on("volume_disk_usage", Ok(MockResponse::DiskUsageMap(sizes)));

        clear_artifacts(&mock, true, false, false, true)
            .await
            .unwrap();

        mock.assert_called("volume_remove", 2);
        mock.assert_called_with("volume_remove", &["mino-cache-npm-abc123"]);
        mock.assert_called_with("volume_remove", &["mino-cache-cargo-def456"]);
    }

    #[tokio::test]
    async fn clear_images_removes_composed_and_base() {
        let images = vec![
            "mino-composed-rust:latest".to_string(),
            "mino-composed-python:latest".to_string(),
        ];

        let mock =
            MockRuntime::new().on("image_list_prefixed", Ok(MockResponse::StringVec(images)));

        clear_artifacts(&mock, false, true, false, true)
            .await
            .unwrap();

        mock.assert_called("container_prune", 1);
        // 2 composed + 1 base image
        mock.assert_called("image_remove", 3);
        mock.assert_called_with("image_remove", &["mino-composed-rust:latest"]);
        mock.assert_called_with("image_remove", &["mino-composed-python:latest"]);
        mock.assert_called_with("image_remove", &[LAYER_BASE_IMAGE]);
    }

    #[tokio::test]
    async fn clear_home_removes_home_volumes() {
        let home_vol = VolumeInfo {
            name: "mino-home-abc123def456".to_string(),
            labels: HashMap::from([
                ("io.mino.home".to_string(), "true".to_string()),
                (
                    "io.mino.home.project".to_string(),
                    "/home/user/project".to_string(),
                ),
            ]),
            mountpoint: None,
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            size_bytes: None,
        };

        let mock = MockRuntime::new().on(
            "volume_list",
            Ok(MockResponse::VolumeInfoVec(vec![home_vol])),
        );

        clear_artifacts(&mock, false, false, true, true)
            .await
            .unwrap();

        mock.assert_called("volume_remove", 1);
        mock.assert_called_with("volume_remove", &["mino-home-abc123def456"]);
    }

    #[tokio::test]
    async fn clear_images_also_removes_base() {
        let images = vec!["mino-composed-abc:latest".to_string()];

        let mock =
            MockRuntime::new().on("image_list_prefixed", Ok(MockResponse::StringVec(images)));

        clear_artifacts(&mock, false, true, false, true)
            .await
            .unwrap();

        mock.assert_called("container_prune", 1);
        mock.assert_called("image_remove", 2);
        mock.assert_called_with("image_remove", &["mino-composed-abc:latest"]);
        mock.assert_called_with("image_remove", &[LAYER_BASE_IMAGE]);
    }

    #[tokio::test]
    async fn gc_dry_run_no_deletes() {
        let mut vol = mino_cache_volume("mino-cache-npm-abc123");
        vol.created_at = Some("2025-01-01T00:00:00Z".to_string());

        let sizes = HashMap::from([("mino-cache-npm-abc123".to_string(), 1024u64)]);

        let mock = MockRuntime::new()
            .on("volume_disk_usage", Ok(MockResponse::DiskUsageMap(sizes)))
            .on("volume_list", Ok(MockResponse::VolumeInfoVec(vec![vol])));

        let config = Config::default();
        gc_caches(&mock, &config, Some(30), true).await.unwrap();

        mock.assert_called("volume_remove", 0);
    }
}
