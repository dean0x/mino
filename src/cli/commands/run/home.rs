//! Home volume setup for persistent per-project home directories

use crate::cli::args::RunArgs;
use crate::config::Config;
use crate::error::MinoResult;
use crate::home::{self, HomeVolume};
use crate::orchestration::ContainerRuntime;
use std::path::Path;
use tracing::debug;

use super::image::LAYER_BASE_IMAGE;

/// Set up a persistent home volume for the project, if applicable.
///
/// Returns `Some("volume_name:/home/developer")` when a home volume should
/// be mounted, or `None` when home volumes are disabled or not applicable.
pub(super) async fn setup_home_volume(
    runtime: &dyn ContainerRuntime,
    args: &RunArgs,
    config: &Config,
    project_dir: &Path,
    image: &str,
) -> MinoResult<Option<String>> {
    // Guard: disabled by CLI flag or config
    if args.no_home || !config.home.enabled {
        debug!("Home volume disabled by flag/config");
        return Ok(None);
    }

    // Guard: only for mino-managed images
    if !is_mino_image(image) {
        debug!("Skipping home volume for custom image: {}", image);
        return Ok(None);
    }

    // Guard: user-specified volume already targets /home/developer
    if has_home_mount(&args.volume, &config.container.volumes) {
        debug!("Skipping home volume: user-specified mount at /home/developer");
        return Ok(None);
    }

    let volume_name = home::home_volume_name(project_dir);

    // Check if volume already exists
    let existing = runtime.volume_inspect(&volume_name).await?;
    if existing.is_some() {
        debug!("Reusing existing home volume: {}", volume_name);
    } else {
        debug!("Creating home volume: {}", volume_name);
        let labels = HomeVolume::labels(project_dir);
        runtime.volume_create(&volume_name, &labels).await?;
    }

    Ok(Some(format!("{}:/home/developer", volume_name)))
}

/// Check whether the resolved image is a mino-managed image.
///
/// Returns true for the GHCR base image and composed layer images.
pub(super) fn is_mino_image(image: &str) -> bool {
    image == LAYER_BASE_IMAGE || image.starts_with("mino-composed-")
}

/// Check whether user-specified volumes include a mount at /home/developer.
pub(super) fn has_home_mount(cli_volumes: &[String], config_volumes: &[String]) -> bool {
    cli_volumes
        .iter()
        .chain(config_volumes.iter())
        .any(|v| v.contains(":/home/developer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::MockRuntime;
    use crate::orchestration::VolumeInfo;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_args() -> RunArgs {
        RunArgs {
            name: None,
            project: None,
            aws: false,
            gcp: false,
            azure: false,
            all_clouds: false,
            no_ssh_agent: false,
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

    // -- setup_home_volume integration tests --

    #[tokio::test]
    async fn setup_creates_volume_on_miss() {
        let mock = Arc::new(MockRuntime::new());
        let args = test_args();
        let config = Config::default();
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(&*mock, &args, &config, &project, LAYER_BASE_IMAGE)
            .await
            .unwrap();

        assert!(result.is_some());
        assert!(result.unwrap().contains(":/home/developer"));
        mock.assert_called("volume_inspect", 1);
        mock.assert_called("volume_create", 1);
    }

    #[tokio::test]
    async fn setup_reuses_existing_volume() {
        use crate::orchestration::mock::MockResponse;
        let vol = VolumeInfo {
            name: "mino-home-existing".to_string(),
            labels: HashMap::new(),
            mountpoint: None,
            created_at: None,
            size_bytes: None,
        };
        let mock = Arc::new(MockRuntime::new().on(
            "volume_inspect",
            Ok(MockResponse::OptionalVolumeInfo(Some(vol))),
        ));
        let args = test_args();
        let config = Config::default();
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(&*mock, &args, &config, &project, LAYER_BASE_IMAGE)
            .await
            .unwrap();

        assert!(result.is_some());
        mock.assert_called("volume_inspect", 1);
        mock.assert_called("volume_create", 0);
    }

    #[tokio::test]
    async fn setup_disabled_by_no_home_flag() {
        let mock = Arc::new(MockRuntime::new());
        let mut args = test_args();
        args.no_home = true;
        let config = Config::default();
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(&*mock, &args, &config, &project, LAYER_BASE_IMAGE)
            .await
            .unwrap();

        assert!(result.is_none());
        mock.assert_called("volume_inspect", 0);
        mock.assert_called("volume_create", 0);
    }

    #[tokio::test]
    async fn setup_disabled_by_config() {
        let mock = Arc::new(MockRuntime::new());
        let args = test_args();
        let mut config = Config::default();
        config.home.enabled = false;
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(&*mock, &args, &config, &project, LAYER_BASE_IMAGE)
            .await
            .unwrap();

        assert!(result.is_none());
        mock.assert_called("volume_inspect", 0);
    }

    #[tokio::test]
    async fn setup_skips_custom_image() {
        let mock = Arc::new(MockRuntime::new());
        let args = test_args();
        let config = Config::default();
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(&*mock, &args, &config, &project, "fedora:43")
            .await
            .unwrap();

        assert!(result.is_none());
        mock.assert_called("volume_inspect", 0);
    }

    #[tokio::test]
    async fn setup_works_with_composed_image() {
        let mock = Arc::new(MockRuntime::new());
        let args = test_args();
        let config = Config::default();
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(
            &*mock,
            &args,
            &config,
            &project,
            "mino-composed-abc123def456",
        )
        .await
        .unwrap();

        assert!(result.is_some());
        mock.assert_called("volume_create", 1);
    }

    #[tokio::test]
    async fn setup_skips_when_user_volume_at_home() {
        let mock = Arc::new(MockRuntime::new());
        let mut args = test_args();
        args.volume = vec!["/my/dir:/home/developer".to_string()];
        let config = Config::default();
        let project = PathBuf::from("/tmp/test-project");

        let result = setup_home_volume(&*mock, &args, &config, &project, LAYER_BASE_IMAGE)
            .await
            .unwrap();

        assert!(result.is_none());
        mock.assert_called("volume_inspect", 0);
    }

    // -- is_mino_image tests --

    #[test]
    fn is_mino_image_base() {
        assert!(is_mino_image(LAYER_BASE_IMAGE));
    }

    #[test]
    fn is_mino_image_composed() {
        assert!(is_mino_image("mino-composed-abc123def456"));
    }

    #[test]
    fn is_mino_image_custom_false() {
        assert!(!is_mino_image("fedora:43"));
        assert!(!is_mino_image("custom:tag"));
        assert!(!is_mino_image("ghcr.io/other/image:latest"));
    }

    // -- has_home_mount tests --

    #[test]
    fn has_home_mount_cli_volume() {
        let cli = vec!["/my/dir:/home/developer".to_string()];
        assert!(has_home_mount(&cli, &[]));
    }

    #[test]
    fn has_home_mount_cli_volume_with_options() {
        let cli = vec!["/my/dir:/home/developer:rw".to_string()];
        assert!(has_home_mount(&cli, &[]));
    }

    #[test]
    fn has_home_mount_config_volume() {
        let config = vec!["/my/dir:/home/developer".to_string()];
        assert!(has_home_mount(&[], &config));
    }

    #[test]
    fn has_home_mount_no_match() {
        let cli = vec!["/my/dir:/workspace".to_string()];
        let config = vec!["/my/dir:/home/other".to_string()];
        assert!(!has_home_mount(&cli, &config));
    }

    #[test]
    fn has_home_mount_empty() {
        assert!(!has_home_mount(&[], &[]));
    }
}
