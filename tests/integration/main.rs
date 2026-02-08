//! Integration tests for Mino

mod cli_tests {
    use assert_cmd::{cargo::cargo_bin_cmd, Command};
    use predicates::prelude::*;

    fn mino() -> Command {
        cargo_bin_cmd!("mino")
    }

    #[test]
    fn help_displays() {
        mino()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Secure AI agent sandbox"));
    }

    #[test]
    fn version_displays() {
        mino()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("mino"));
    }

    #[test]
    fn status_runs() {
        // Status may fail if OrbStack isn't installed, but should not panic
        let _ = mino().arg("status").assert();
    }

    #[test]
    fn list_empty() {
        mino().args(["list"]).assert().success().stdout(
            predicate::str::contains("No active sessions").or(predicate::str::contains("NAME")),
        );
    }

    #[test]
    fn config_path() {
        mino()
            .args(["config", "path"])
            .assert()
            .success()
            .stdout(predicate::str::contains("config.toml"));
    }

    #[test]
    fn config_show() {
        mino()
            .args(["config", "show"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[general]"));
    }

    #[test]
    fn stop_missing_session() {
        mino()
            .args(["stop", "nonexistent-session"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Session not found"));
    }

    #[test]
    fn logs_missing_session() {
        mino()
            .args(["logs", "nonexistent-session"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Session not found"));
    }

    #[test]
    fn setup_check_runs() {
        // Setup check should run without error (may report issues but shouldn't panic)
        mino()
            .args(["setup", "--check"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Mino Setup"));
    }

    #[test]
    fn setup_help() {
        mino()
            .args(["setup", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Interactive setup wizard"));
    }
}
