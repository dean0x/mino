//! Cache command - manage dependency caches

use crate::cache::{
    detect_lockfiles, format_bytes, gb_to_bytes, CacheSizeStatus, CacheState, CacheVolume,
};
use crate::cli::args::{CacheAction, CacheArgs, OutputFormat};
use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use crate::ui::{self, UiContext};
use chrono::Utc;
use console::style;
use std::env;
use std::path::PathBuf;
use tracing::debug;

/// Execute the cache command
pub async fn execute(args: CacheArgs, config: &Config) -> MinotaurResult<()> {
    let runtime = create_runtime(config)?;

    match args.action {
        CacheAction::List { format } => list_caches(&*runtime, format, config).await,
        CacheAction::Info { project } => show_project_info(&*runtime, project, config).await,
        CacheAction::Gc { days, dry_run } => gc_caches(&*runtime, config, days, dry_run).await,
        CacheAction::Clear { all: _, yes } => clear_all_caches(&*runtime, yes).await,
    }
}

/// List all cache volumes with sizes
async fn list_caches(
    runtime: &dyn ContainerRuntime,
    format: OutputFormat,
    config: &Config,
) -> MinotaurResult<()> {
    let volumes = runtime.volume_list("minotaur-cache-").await?;

    if volumes.is_empty() {
        match format {
            OutputFormat::Json => println!("{{\"caches\":[]}}"),
            OutputFormat::Plain => {}
            OutputFormat::Table => println!("No cache volumes found."),
        }
        return Ok(());
    }

    // Get disk usage for all cache volumes
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;

    // Parse into CacheVolume structs with sizes
    let caches: Vec<(CacheVolume, u64)> = volumes
        .iter()
        .filter_map(|v| {
            CacheVolume::from_labels(&v.name, &v.labels)
                .map(|c| (c.clone(), *sizes.get(&c.name).unwrap_or(&0)))
        })
        .collect();

    // Calculate total size
    let total_size: u64 = caches.iter().map(|(_, s)| s).sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);

    match format {
        OutputFormat::Table => print_cache_table(&caches, total_size, limit_bytes),
        OutputFormat::Json => print_cache_json(&caches, total_size, limit_bytes)?,
        OutputFormat::Plain => print_cache_plain(&caches),
    }

    Ok(())
}

fn print_cache_table(caches: &[(CacheVolume, u64)], total_size: u64, limit_bytes: u64) {
    let ctx = UiContext::detect();

    ui::intro(&ctx, "Cache Volumes");

    println!(
        "{:<40} {:<10} {:<10} {:<10} {:<16}",
        "VOLUME", "ECOSYSTEM", "STATE", "SIZE", "CREATED"
    );
    println!("{}", "-".repeat(90));

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
            "{:<40} {:<10} {:<10} {:<10} {:<16}",
            cache.name, cache.ecosystem, state_display, size_display, created
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
            "{} {} - consider running: minotaur cache gc",
            style("!").yellow(),
            total_display
        ),
        CacheSizeStatus::Exceeded => println!(
            "{} {} - run: minotaur cache gc",
            style("!").red().bold(),
            total_display
        ),
    }

    println!("{} cache(s)", caches.len());
}

fn print_cache_json(
    caches: &[(CacheVolume, u64)],
    total_size: u64,
    limit_bytes: u64,
) -> MinotaurResult<()> {
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
    struct Output {
        caches: Vec<CacheJson>,
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

    let output = Output {
        caches: json_caches,
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
) -> MinotaurResult<()> {
    let ctx = UiContext::detect();

    let project_dir = match project {
        Some(p) => p.canonicalize().unwrap_or(p),
        None => {
            env::current_dir().map_err(|e| MinotaurError::io("getting current directory", e))?
        }
    };

    ui::intro(&ctx, "Project Cache Info");
    ui::key_value(&ctx, "Project", &project_dir.display().to_string());

    // Detect lockfiles
    let lockfiles = detect_lockfiles(&project_dir)?;

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
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;

    // Check cache states
    ui::section(&ctx, "Cache status");
    let mut project_total: u64 = 0;

    for info in &lockfiles {
        let volume_name = info.volume_name();
        let volume_info = runtime.volume_inspect(&volume_name).await?;
        let size = sizes.get(&volume_name).copied().unwrap_or(0);
        project_total += size;

        match volume_info {
            Some(v) => {
                let cache = CacheVolume::from_labels(&v.name, &v.labels);
                match cache.map(|c| c.state) {
                    Some(CacheState::Complete) => {
                        let size_str = if size > 0 {
                            format!(" ({})", format_bytes(size))
                        } else {
                            String::new()
                        };
                        ui::step_ok_detail(
                            &ctx,
                            &format!("{}", info.ecosystem),
                            &format!("complete (ro){}", size_str),
                        );
                    }
                    Some(CacheState::Building) => {
                        let size_str = if size > 0 {
                            format!(" ({})", format_bytes(size))
                        } else {
                            String::new()
                        };
                        ui::step_warn_hint(
                            &ctx,
                            &format!("{}", info.ecosystem),
                            &format!("building (rw){}", size_str),
                        );
                    }
                    _ => {
                        ui::step_info(&ctx, &format!("{}: unknown state", info.ecosystem));
                    }
                }
            }
            None => {
                ui::step_info(&ctx, &format!("{}: miss (will create)", info.ecosystem));
            }
        }
    }

    // Show total cache usage
    let all_sizes = runtime.volume_disk_usage("minotaur-cache-").await?;
    let total_size: u64 = all_sizes.values().sum();
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
) -> MinotaurResult<()> {
    let ctx = UiContext::detect();
    let gc_days = days_override.unwrap_or(config.cache.gc_days);

    // Get current cache size
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;
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

    let volumes = runtime.volume_list("minotaur-cache-").await?;
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

    ui::section(&ctx, &format!("Found {} cache(s) to remove ({})", to_remove.len(), format_bytes(bytes_to_free)));

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

    if dry_run {
        println!();
        ui::note(&ctx, "Dry run", "No caches removed.");
        return Ok(());
    }

    println!();
    let mut spinner = ui::TaskSpinner::new(&ctx);
    spinner.start("Removing caches...");

    let mut removed = 0;
    for (cache, _) in to_remove {
        debug!("Removing cache: {}", cache.name);
        runtime.volume_remove(&cache.name).await?;
        removed += 1;
    }

    spinner.stop(&format!(
        "Removed {} cache(s), freed {}",
        removed,
        format_bytes(bytes_to_free)
    ));

    Ok(())
}

/// Clear all caches
async fn clear_all_caches(
    runtime: &dyn ContainerRuntime,
    skip_confirm: bool,
) -> MinotaurResult<()> {
    let ctx = UiContext::detect();
    let volumes = runtime.volume_list("minotaur-cache-").await?;

    if volumes.is_empty() {
        ui::intro(&ctx, "Cache Clear");
        ui::step_info(&ctx, "No cache volumes to clear.");
        return Ok(());
    }

    // Get sizes
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;
    let total_size: u64 = sizes.values().sum();

    ui::intro(&ctx, "Cache Clear");
    ui::step_warn(
        &ctx,
        &format!(
            "This will remove {} cache volume(s) ({})",
            volumes.len(),
            format_bytes(total_size)
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

    if !skip_confirm {
        let confirmed = ui::confirm(&ctx, "Are you sure you want to clear all caches?", false).await?;
        if !confirmed {
            ui::outro_warn(&ctx, "Aborted.");
            return Ok(());
        }
    }

    let mut spinner = ui::TaskSpinner::new(&ctx);
    spinner.start("Clearing caches...");

    let mut removed = 0;
    for vol in volumes {
        runtime.volume_remove(&vol.name).await?;
        removed += 1;
    }

    spinner.stop(&format!(
        "Cleared {} cache(s), freed {}",
        removed,
        format_bytes(total_size)
    ));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::format_bytes;

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
}
