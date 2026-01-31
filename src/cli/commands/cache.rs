//! Cache command - manage dependency caches

use crate::cache::{detect_lockfiles, CacheState, CacheVolume};
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
        CacheAction::List { format } => list_caches(&*runtime, format).await,
        CacheAction::Info { project } => show_project_info(&*runtime, project, config).await,
        CacheAction::Gc { days, dry_run } => gc_caches(&*runtime, config, days, dry_run).await,
        CacheAction::Clear { all: _, yes } => clear_all_caches(&*runtime, yes).await,
    }
}

/// List all cache volumes
async fn list_caches(runtime: &dyn ContainerRuntime, format: OutputFormat) -> MinotaurResult<()> {
    let volumes = runtime.volume_list("minotaur-cache-").await?;

    if volumes.is_empty() {
        println!("No cache volumes found.");
        return Ok(());
    }

    // Parse into CacheVolume structs
    let caches: Vec<CacheVolume> = volumes
        .iter()
        .filter_map(|v| CacheVolume::from_labels(&v.name, &v.labels))
        .collect();

    match format {
        OutputFormat::Table => print_cache_table(&caches),
        OutputFormat::Json => print_cache_json(&caches)?,
        OutputFormat::Plain => print_cache_plain(&caches),
    }

    Ok(())
}

fn print_cache_table(caches: &[CacheVolume]) {
    println!(
        "{:<40} {:<10} {:<10} {:<20}",
        "VOLUME", "ECOSYSTEM", "STATE", "CREATED"
    );
    println!("{}", "-".repeat(80));

    for cache in caches {
        let state_display = match cache.state {
            CacheState::Complete => style("complete").green().to_string(),
            CacheState::Building => style("building").yellow().to_string(),
            CacheState::Miss => style("miss").dim().to_string(),
        };

        let created = cache.created_at.format("%Y-%m-%d %H:%M").to_string();

        println!(
            "{:<40} {:<10} {:<10} {:<20}",
            cache.name, cache.ecosystem, state_display, created
        );
    }

    println!();
    println!("Total: {} cache(s)", caches.len());
}

fn print_cache_json(caches: &[CacheVolume]) -> MinotaurResult<()> {
    #[derive(serde::Serialize)]
    struct CacheJson {
        name: String,
        ecosystem: String,
        hash: String,
        state: String,
        created_at: String,
    }

    let json_caches: Vec<CacheJson> = caches
        .iter()
        .map(|c| CacheJson {
            name: c.name.clone(),
            ecosystem: c.ecosystem.to_string(),
            hash: c.hash.clone(),
            state: c.state.to_string(),
            created_at: c.created_at.to_rfc3339(),
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&json_caches)?);
    Ok(())
}

fn print_cache_plain(caches: &[CacheVolume]) {
    for cache in caches {
        println!("{}", cache.name);
    }
}

/// Show cache info for a specific project
async fn show_project_info(
    runtime: &dyn ContainerRuntime,
    project: Option<PathBuf>,
    _config: &Config,
) -> MinotaurResult<()> {
    let project_dir = match project {
        Some(p) => p.canonicalize().unwrap_or(p),
        None => env::current_dir().map_err(|e| MinotaurError::io("getting current directory", e))?,
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

    // Check cache states
    println!("Cache status:");
    for info in &lockfiles {
        let volume_name = info.volume_name();
        let volume_info = runtime.volume_inspect(&volume_name).await?;

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

        println!(
            "  {} {} {} [{}]",
            state_style,
            info.ecosystem,
            volume_name,
            state
        );
    }

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

    if gc_days == 0 {
        println!("Cache GC is disabled (gc_days = 0)");
        return Ok(());
    }

    let volumes = runtime.volume_list("minotaur-cache-").await?;
    let caches: Vec<CacheVolume> = volumes
        .iter()
        .filter_map(|v| CacheVolume::from_labels(&v.name, &v.labels))
        .collect();

    // Find caches to remove
    let to_remove: Vec<&CacheVolume> = caches
        .iter()
        .filter(|c| c.is_older_than_days(gc_days))
        .collect();

    if to_remove.is_empty() {
        println!("No caches older than {} days.", gc_days);
        return Ok(());
    }

    println!(
        "Found {} cache(s) older than {} days:",
        to_remove.len(),
        gc_days
    );

    for cache in &to_remove {
        let age_days = (Utc::now() - cache.created_at).num_days();
        println!(
            "  {} {} ({} days old)",
            style("•").red(),
            cache.name,
            age_days
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
    for cache in to_remove {
        debug!("Removing cache: {}", cache.name);
        runtime.volume_remove(&cache.name).await?;
        removed += 1;
    }

    println!("{} removed {} cache(s)", style("✓").green(), removed);

    Ok(())
}

/// Clear all caches
async fn clear_all_caches(runtime: &dyn ContainerRuntime, skip_confirm: bool) -> MinotaurResult<()> {
    let volumes = runtime.volume_list("minotaur-cache-").await?;

    if volumes.is_empty() {
        println!("No cache volumes to clear.");
        return Ok(());
    }

    println!("This will remove {} cache volume(s):", volumes.len());
    for vol in &volumes {
        println!("  {} {}", style("•").red(), vol.name);
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

    println!("{} cleared {} cache(s)", style("✓").green(), removed);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests would require a running Podman instance
    // Unit tests for formatting functions

    #[test]
    fn cache_state_display() {
        assert_eq!(CacheState::Complete.to_string(), "complete");
        assert_eq!(CacheState::Building.to_string(), "building");
        assert_eq!(CacheState::Miss.to_string(), "miss");
    }
}
