//! Cache command - manage dependency caches

use crate::cache::{
    detect_lockfiles, format_bytes, gb_to_bytes, CacheSizeStatus, CacheState, CacheVolume,
};
use crate::cli::args::{CacheAction, CacheArgs, OutputFormat};
use crate::config::Config;
use crate::error::{MinotaurError, MinotaurResult};
use crate::orchestration::{create_runtime, ContainerRuntime};
use chrono::Utc;
use console::style;
use std::env;
use std::io::{self, Write};
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
        println!("No cache volumes found.");
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
    let project_dir = match project {
        Some(p) => p.canonicalize().unwrap_or(p),
        None => {
            env::current_dir().map_err(|e| MinotaurError::io("getting current directory", e))?
        }
    };

    println!("Project: {}", project_dir.display());
    println!();

    // Detect lockfiles
    let lockfiles = detect_lockfiles(&project_dir)?;

    if lockfiles.is_empty() {
        println!("No lockfiles detected in this project.");
        return Ok(());
    }

    println!("Detected lockfiles:");
    for info in &lockfiles {
        println!(
            "  {} {} (hash: {})",
            style("•").cyan(),
            info.path.file_name().unwrap_or_default().to_string_lossy(),
            &info.hash
        );
    }
    println!();

    // Get disk usage
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;

    // Check cache states
    println!("Cache status:");
    let mut project_total: u64 = 0;

    for info in &lockfiles {
        let volume_name = info.volume_name();
        let volume_info = runtime.volume_inspect(&volume_name).await?;
        let size = sizes.get(&volume_name).copied().unwrap_or(0);
        project_total += size;

        let (state, state_style) = match volume_info {
            Some(v) => {
                let cache = CacheVolume::from_labels(&v.name, &v.labels);
                match cache.map(|c| c.state) {
                    Some(CacheState::Complete) => ("complete (ro)", style("✓").green()),
                    Some(CacheState::Building) => ("building (rw)", style("~").yellow()),
                    _ => ("unknown", style("?").dim()),
                }
            }
            None => ("miss (will create)", style("○").dim()),
        };

        let size_str = if size > 0 {
            format!(" ({})", format_bytes(size))
        } else {
            String::new()
        };

        println!(
            "  {} {} [{}]{}",
            state_style, info.ecosystem, state, size_str
        );
    }

    println!();

    // Show total cache usage
    let all_sizes = runtime.volume_disk_usage("minotaur-cache-").await?;
    let total_size: u64 = all_sizes.values().sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);
    let percent = CacheSizeStatus::percentage(total_size, limit_bytes);

    println!(
        "Project cache size: {}",
        if project_total > 0 {
            format_bytes(project_total)
        } else {
            "0 B".to_string()
        }
    );
    println!(
        "Total cache usage:  {} / {} ({:.0}%)",
        format_bytes(total_size),
        format_bytes(limit_bytes),
        percent
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
    let gc_days = days_override.unwrap_or(config.cache.gc_days);

    // Get current cache size
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;
    let total_size: u64 = sizes.values().sum();
    let limit_bytes = gb_to_bytes(config.cache.max_total_gb);

    println!(
        "Current cache usage: {} / {} ({:.0}%)",
        format_bytes(total_size),
        format_bytes(limit_bytes),
        CacheSizeStatus::percentage(total_size, limit_bytes)
    );
    println!();

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
            println!("No caches older than {} days.", gc_days);
        } else {
            println!("Cache GC by age is disabled (gc_days = 0).");
        }
        return Ok(());
    }

    let bytes_to_free: u64 = to_remove.iter().map(|(_, s)| s).sum();

    println!(
        "Found {} cache(s) to remove ({}):",
        to_remove.len(),
        format_bytes(bytes_to_free)
    );

    for (cache, size) in &to_remove {
        let age_days = (Utc::now() - cache.created_at).num_days();
        let size_str = if *size > 0 {
            format!(" ({})", format_bytes(*size))
        } else {
            String::new()
        };
        println!(
            "  {} {} - {} days old{}",
            style("•").red(),
            cache.name,
            age_days,
            size_str
        );
    }

    if dry_run {
        println!();
        println!("Dry run - no caches removed.");
        return Ok(());
    }

    println!();
    print!("Removing caches... ");
    let _ = io::stdout().flush();

    let mut removed = 0;
    for (cache, _) in to_remove {
        debug!("Removing cache: {}", cache.name);
        runtime.volume_remove(&cache.name).await?;
        removed += 1;
    }

    println!(
        "{} removed {} cache(s), freed {}",
        style("✓").green(),
        removed,
        format_bytes(bytes_to_free)
    );

    Ok(())
}

/// Clear all caches
async fn clear_all_caches(
    runtime: &dyn ContainerRuntime,
    skip_confirm: bool,
) -> MinotaurResult<()> {
    let volumes = runtime.volume_list("minotaur-cache-").await?;

    if volumes.is_empty() {
        println!("No cache volumes to clear.");
        return Ok(());
    }

    // Get sizes
    let sizes = runtime.volume_disk_usage("minotaur-cache-").await?;
    let total_size: u64 = sizes.values().sum();

    println!(
        "This will remove {} cache volume(s) ({}):",
        volumes.len(),
        format_bytes(total_size)
    );
    for vol in &volumes {
        let size = sizes.get(&vol.name).copied().unwrap_or(0);
        let size_str = if size > 0 {
            format!(" ({})", format_bytes(size))
        } else {
            String::new()
        };
        println!("  {} {}{}", style("•").red(), vol.name, size_str);
    }
    println!();

    if !skip_confirm {
        print!("Are you sure? [y/N] ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            println!("Failed to read input, aborting.");
            return Ok(());
        }

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    print!("Clearing caches... ");
    let _ = io::stdout().flush();

    let mut removed = 0;
    for vol in volumes {
        runtime.volume_remove(&vol.name).await?;
        removed += 1;
    }

    println!(
        "{} cleared {} cache(s), freed {}",
        style("✓").green(),
        removed,
        format_bytes(total_size)
    );

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
