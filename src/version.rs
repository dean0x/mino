//! Version awareness for mino run
//!
//! Two lightweight checks:
//! 1. Stale image warning — after a version upgrade, cached composed images may need rebuilding
//! 2. Update check — periodic (24h) check for newer stable releases on GitHub
//!
//! Both are silent-on-failure and never block the primary workflow.

use crate::config::{schema::Config, ConfigManager};
use crate::orchestration::ContainerRuntime;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

const STATE_FILENAME: &str = "version_state.json";
const GITHUB_RELEASES_URL: &str = "https://api.github.com/repos/dean0x/mino/releases/latest";

/// Persisted version state at `~/.local/share/mino/version_state.json`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VersionState {
    pub installed_version: Option<String>,
    pub last_update_check: Option<DateTime<Utc>>,
    pub latest_available: Option<String>,
}

/// Info about stale composed images after a mino version change
pub struct StaleImageInfo {
    pub old: String,
    pub new: String,
}

/// Info about an available mino update
pub struct UpdateInfo {
    pub latest: String,
    pub current: String,
}

/// How mino was installed (for upgrade hints)
pub enum InstallMethod {
    Homebrew,
    Cargo,
    Npm,
    Unknown,
}

// --- Pure functions ---

/// Returns `Some` if stored version differs from current (upgrade or downgrade).
/// Returns `None` on first run (no stored version = no baseline).
pub fn should_warn_stale_images(
    state: &VersionState,
    current_version: &str,
) -> Option<StaleImageInfo> {
    let stored = state.installed_version.as_deref()?;
    if stored == current_version {
        return None;
    }
    Some(StaleImageInfo {
        old: stored.to_string(),
        new: current_version.to_string(),
    })
}

/// Returns true if no previous update check or >24h since last check.
pub fn should_check_update(state: &VersionState) -> bool {
    let Some(last_check) = state.last_update_check else {
        return true;
    };
    Utc::now() - last_check > chrono::Duration::hours(24)
}

/// Returns true if `latest` is newer than `current` per semver.
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let Ok(latest_ver) = semver::Version::parse(latest) else {
        return false;
    };
    let Ok(current_ver) = semver::Version::parse(current) else {
        return false;
    };
    latest_ver > current_ver
}

/// Extracts version string from GitHub releases/latest JSON response.
/// Strips leading `v` prefix if present.
pub fn parse_github_release(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    let version_str = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(version_str).ok()?;
    Some(version_str.to_string())
}

/// Detects how mino was installed based on the executable path.
pub fn detect_install_method() -> InstallMethod {
    let Ok(exe) = std::env::current_exe() else {
        return InstallMethod::Unknown;
    };
    let path = exe.to_string_lossy();
    if path.contains("/opt/homebrew/") || path.contains("/usr/local/Cellar/") {
        InstallMethod::Homebrew
    } else if path.contains(".cargo/") {
        InstallMethod::Cargo
    } else if path.contains("node_modules") {
        InstallMethod::Npm
    } else {
        InstallMethod::Unknown
    }
}

/// Returns an install-method-specific update command hint.
pub fn update_hint(method: &InstallMethod) -> &'static str {
    match method {
        InstallMethod::Homebrew => "Update: brew upgrade mino",
        InstallMethod::Cargo => "Update: cargo install mino",
        InstallMethod::Npm => "Update: npm update -g mino",
        InstallMethod::Unknown => "Visit https://github.com/dean0x/mino/releases",
    }
}

// --- State IO ---

fn state_path() -> PathBuf {
    ConfigManager::state_dir().join(STATE_FILENAME)
}

async fn load_state_from(path: &Path) -> VersionState {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return VersionState::default(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

async fn save_state_to(path: &Path, state: &VersionState) {
    if let Some(parent) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            warn!("Failed to create state directory: {}", e);
            return;
        }
    }
    let json = match serde_json::to_string_pretty(state) {
        Ok(j) => j,
        Err(e) => {
            warn!("Failed to serialize version state: {}", e);
            return;
        }
    };
    if let Err(e) = tokio::fs::write(path, json).await {
        warn!("Failed to write version state: {}", e);
    }
}

// --- Public async functions ---

/// Check if cached composed images may be stale after a version upgrade.
///
/// Loads persisted state, compares stored version against current. Only queries
/// the runtime for composed images when a version change is detected (avoids
/// unnecessary Podman subprocess on every run). Always writes current version
/// to state file to bootstrap baseline on first run.
pub async fn check_stale_images(runtime: &dyn ContainerRuntime) -> Option<StaleImageInfo> {
    check_stale_images_inner(runtime, &state_path()).await
}

async fn check_stale_images_inner(
    runtime: &dyn ContainerRuntime,
    path: &Path,
) -> Option<StaleImageInfo> {
    let state = load_state_from(path).await;
    let current = env!("CARGO_PKG_VERSION");

    let result = should_warn_stale_images(&state, current);

    // Only query composed images if version actually changed
    let result = match result {
        Some(info) => match runtime.image_list_prefixed("mino-composed-").await {
            Ok(images) if !images.is_empty() => Some(info),
            Ok(_) => None,
            Err(e) => {
                warn!("Failed to list composed images: {}", e);
                None
            }
        },
        None => None,
    };

    // Always write current version to bootstrap baseline
    let updated = VersionState {
        installed_version: Some(current.to_string()),
        ..state
    };
    save_state_to(path, &updated).await;

    result
}

/// Check for a newer mino release on GitHub.
///
/// Rate-limited to once per 24 hours. Between checks, uses cached
/// `latest_available` from state file. Gated on `config.general.update_check`.
/// HTTP request uses a 3-second global timeout via ureq in `spawn_blocking`.
pub async fn check_for_update(config: &Config) -> Option<UpdateInfo> {
    check_for_update_inner(config, &state_path()).await
}

async fn check_for_update_inner(config: &Config, path: &Path) -> Option<UpdateInfo> {
    if !config.general.update_check {
        return None;
    }

    let mut state = load_state_from(path).await;
    let current = env!("CARGO_PKG_VERSION");

    if !should_check_update(&state) {
        // Use cached result
        let latest = state.latest_available.as_deref()?;
        if is_newer_version(latest, current) {
            return Some(UpdateInfo {
                latest: latest.to_string(),
                current: current.to_string(),
            });
        }
        return None;
    }

    // Perform HTTP check
    let body = match tokio::task::spawn_blocking(fetch_latest_release).await {
        Ok(Ok(body)) => body,
        Ok(Err(e)) => {
            warn!("Update check failed: {}", e);
            return None;
        }
        Err(e) => {
            warn!("Update check task failed: {}", e);
            return None;
        }
    };

    let latest = match parse_github_release(&body) {
        Some(v) => v,
        None => {
            warn!("Failed to parse GitHub release response");
            return None;
        }
    };

    state.last_update_check = Some(Utc::now());
    state.latest_available = Some(latest.clone());
    save_state_to(path, &state).await;

    if is_newer_version(&latest, current) {
        Some(UpdateInfo {
            latest,
            current: current.to_string(),
        })
    } else {
        None
    }
}

fn fetch_latest_release() -> Result<String, String> {
    use std::time::Duration;
    use ureq::Agent;

    let config = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(3)))
        .build();
    let agent: Agent = config.new_agent();

    let body: String = agent
        .get(GITHUB_RELEASES_URL)
        .header("User-Agent", &format!("mino/{}", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github.v3+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::{MockResponse, MockRuntime};
    use tempfile::TempDir;

    // --- Pure function tests ---

    #[test]
    fn stale_images_version_changed() {
        let state = VersionState {
            installed_version: Some("1.3.0".to_string()),
            ..Default::default()
        };
        let result = should_warn_stale_images(&state, "1.4.0").unwrap();
        assert_eq!(result.old, "1.3.0");
        assert_eq!(result.new, "1.4.0");
    }

    #[test]
    fn stale_images_same_version() {
        let state = VersionState {
            installed_version: Some("1.4.0".to_string()),
            ..Default::default()
        };
        assert!(should_warn_stale_images(&state, "1.4.0").is_none());
    }

    #[test]
    fn stale_images_no_stored_version() {
        let state = VersionState::default();
        assert!(should_warn_stale_images(&state, "1.4.0").is_none());
    }

    #[test]
    fn stale_images_downgrade() {
        let state = VersionState {
            installed_version: Some("1.5.0".to_string()),
            ..Default::default()
        };
        let result = should_warn_stale_images(&state, "1.4.0").unwrap();
        assert_eq!(result.old, "1.5.0");
        assert_eq!(result.new, "1.4.0");
    }

    #[test]
    fn check_update_no_previous() {
        let state = VersionState::default();
        assert!(should_check_update(&state));
    }

    #[test]
    fn check_update_over_24h() {
        let state = VersionState {
            last_update_check: Some(Utc::now() - chrono::Duration::hours(25)),
            ..Default::default()
        };
        assert!(should_check_update(&state));
    }

    #[test]
    fn check_update_within_24h() {
        let state = VersionState {
            last_update_check: Some(Utc::now() - chrono::Duration::hours(1)),
            ..Default::default()
        };
        assert!(!should_check_update(&state));
    }

    #[test]
    fn newer_version_detected() {
        assert!(is_newer_version("2.0.0", "1.4.1"));
        assert!(is_newer_version("1.5.0", "1.4.1"));
        assert!(is_newer_version("1.4.2", "1.4.1"));
    }

    #[test]
    fn same_version_not_newer() {
        assert!(!is_newer_version("1.4.1", "1.4.1"));
    }

    #[test]
    fn older_version_not_newer() {
        assert!(!is_newer_version("1.3.0", "1.4.1"));
    }

    #[test]
    fn prerelease_not_newer_than_release() {
        assert!(!is_newer_version("1.4.1-alpha", "1.4.1"));
    }

    #[test]
    fn invalid_version_not_newer() {
        assert!(!is_newer_version("not-a-version", "1.4.1"));
        assert!(!is_newer_version("1.5.0", "not-a-version"));
    }

    #[test]
    fn parse_release_valid() {
        let json = r#"{"tag_name": "v1.5.0", "name": "Release 1.5.0"}"#;
        assert_eq!(parse_github_release(json), Some("1.5.0".to_string()));
    }

    #[test]
    fn parse_release_no_v_prefix() {
        let json = r#"{"tag_name": "1.5.0"}"#;
        assert_eq!(parse_github_release(json), Some("1.5.0".to_string()));
    }

    #[test]
    fn parse_release_missing_tag() {
        let json = r#"{"name": "Release"}"#;
        assert!(parse_github_release(json).is_none());
    }

    #[test]
    fn parse_release_empty_object() {
        assert!(parse_github_release("{}").is_none());
    }

    #[test]
    fn parse_release_invalid_json() {
        assert!(parse_github_release("not json").is_none());
    }

    #[test]
    fn parse_release_invalid_version() {
        let json = r#"{"tag_name": "not-semver"}"#;
        assert!(parse_github_release(json).is_none());
    }

    #[test]
    fn version_state_serde_roundtrip() {
        let state = VersionState {
            installed_version: Some("1.4.1".to_string()),
            last_update_check: Some(Utc::now()),
            latest_available: Some("1.5.0".to_string()),
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: VersionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.installed_version, state.installed_version);
        assert_eq!(parsed.latest_available, state.latest_available);
    }

    #[test]
    fn version_state_empty_json() {
        let state: VersionState = serde_json::from_str("{}").unwrap();
        assert!(state.installed_version.is_none());
        assert!(state.last_update_check.is_none());
        assert!(state.latest_available.is_none());
    }

    #[test]
    fn version_state_partial_json() {
        let state: VersionState =
            serde_json::from_str(r#"{"installed_version": "1.4.0"}"#).unwrap();
        assert_eq!(state.installed_version.as_deref(), Some("1.4.0"));
        assert!(state.last_update_check.is_none());
    }

    #[test]
    fn version_state_corrupt_returns_error() {
        let result: Result<VersionState, _> = serde_json::from_str("not json");
        assert!(result.is_err());
    }

    #[test]
    fn update_hint_homebrew() {
        assert!(update_hint(&InstallMethod::Homebrew).contains("brew"));
    }

    #[test]
    fn update_hint_cargo() {
        assert!(update_hint(&InstallMethod::Cargo).contains("cargo install"));
    }

    #[test]
    fn update_hint_npm() {
        assert!(update_hint(&InstallMethod::Npm).contains("npm"));
    }

    #[test]
    fn update_hint_unknown() {
        assert!(update_hint(&InstallMethod::Unknown).contains("github.com"));
    }

    // --- State IO tests ---

    #[tokio::test]
    async fn load_nonexistent_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let state = load_state_from(&path).await;
        assert!(state.installed_version.is_none());
    }

    #[tokio::test]
    async fn save_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        let state = VersionState {
            installed_version: Some("1.4.1".to_string()),
            last_update_check: Some(Utc::now()),
            latest_available: Some("1.5.0".to_string()),
        };
        save_state_to(&path, &state).await;
        let loaded = load_state_from(&path).await;
        assert_eq!(loaded.installed_version, state.installed_version);
        assert_eq!(loaded.latest_available, state.latest_available);
    }

    #[tokio::test]
    async fn load_corrupt_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("corrupt.json");
        tokio::fs::write(&path, "not json").await.unwrap();
        let state = load_state_from(&path).await;
        assert!(state.installed_version.is_none());
    }

    #[tokio::test]
    async fn first_run_bootstraps_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let mock = MockRuntime::new();
        let result = check_stale_images_inner(&mock, &path).await;
        assert!(result.is_none());

        let state = load_state_from(&path).await;
        assert_eq!(
            state.installed_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        mock.assert_called("image_list_prefixed", 0);
    }

    // --- Integration tests with MockRuntime ---

    #[tokio::test]
    async fn stale_check_version_changed_with_images() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let state = VersionState {
            installed_version: Some("1.0.0".to_string()),
            ..Default::default()
        };
        save_state_to(&path, &state).await;

        let mock = MockRuntime::new().on(
            "image_list_prefixed",
            Ok(MockResponse::StringVec(vec![
                "mino-composed-abc123".to_string()
            ])),
        );

        let result = check_stale_images_inner(&mock, &path).await;
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.old, "1.0.0");
        assert_eq!(info.new, env!("CARGO_PKG_VERSION"));

        mock.assert_called("image_list_prefixed", 1);
        mock.assert_called_with("image_list_prefixed", &["mino-composed-"]);
    }

    #[tokio::test]
    async fn stale_check_version_changed_no_images() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let state = VersionState {
            installed_version: Some("1.0.0".to_string()),
            ..Default::default()
        };
        save_state_to(&path, &state).await;

        let mock = MockRuntime::new();
        let result = check_stale_images_inner(&mock, &path).await;
        assert!(result.is_none());

        mock.assert_called("image_list_prefixed", 1);

        let updated = load_state_from(&path).await;
        assert_eq!(
            updated.installed_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[tokio::test]
    async fn stale_check_same_version_skips_runtime() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let state = VersionState {
            installed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Default::default()
        };
        save_state_to(&path, &state).await;

        let mock = MockRuntime::new();
        let result = check_stale_images_inner(&mock, &path).await;
        assert!(result.is_none());

        mock.assert_called("image_list_prefixed", 0);
    }

    #[tokio::test]
    async fn stale_check_image_list_error_silent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let state = VersionState {
            installed_version: Some("1.0.0".to_string()),
            ..Default::default()
        };
        save_state_to(&path, &state).await;

        let mock = MockRuntime::new().on_err(
            "image_list_prefixed",
            crate::error::MinoError::Internal("test error".to_string()),
        );

        let result = check_stale_images_inner(&mock, &path).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn update_check_disabled_by_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let mut config = Config::default();
        config.general.update_check = false;

        let result = check_for_update_inner(&config, &path).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn update_check_cached_newer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let state = VersionState {
            installed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            last_update_check: Some(Utc::now()),
            latest_available: Some("99.0.0".to_string()),
        };
        save_state_to(&path, &state).await;

        let config = Config::default();
        let result = check_for_update_inner(&config, &path).await;
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.latest, "99.0.0");
        assert_eq!(info.current, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn update_check_cached_same() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let state = VersionState {
            installed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            last_update_check: Some(Utc::now()),
            latest_available: Some(env!("CARGO_PKG_VERSION").to_string()),
        };
        save_state_to(&path, &state).await;

        let config = Config::default();
        let result = check_for_update_inner(&config, &path).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn update_check_no_cached_result() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        // Within 24h but no latest_available cached
        let state = VersionState {
            installed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            last_update_check: Some(Utc::now()),
            latest_available: None,
        };
        save_state_to(&path, &state).await;

        let config = Config::default();
        let result = check_for_update_inner(&config, &path).await;
        assert!(result.is_none());
    }
}
