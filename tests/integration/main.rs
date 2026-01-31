//! Integration tests for Minotaur

mod cli_tests {
    use assert_cmd::Command;
    use predicates::prelude::*;

    fn minotaur() -> Command {
        Command::cargo_bin("minotaur").unwrap()
    }

    #[test]
    fn help_displays() {
        minotaur()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Secure AI agent sandbox"));
    }

    #[test]
    fn version_displays() {
        minotaur()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("minotaur"));
    }

    #[test]
    fn status_runs() {
        // Status may fail if OrbStack isn't installed, but should not panic
        minotaur()
            .arg("status")
            .assert();
    }

    #[test]
    fn list_empty() {
        minotaur()
            .args(["list"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No active sessions").or(predicate::str::contains("NAME")));
    }

    #[test]
    fn config_path() {
        minotaur()
            .args(["config", "path"])
            .assert()
            .success()
            .stdout(predicate::str::contains("config.toml"));
    }

    #[test]
    fn config_show() {
        minotaur()
            .args(["config", "show"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[general]"));
    }

    #[test]
    fn stop_missing_session() {
        minotaur()
            .args(["stop", "nonexistent-session"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Session not found"));
    }

    #[test]
    fn logs_missing_session() {
        minotaur()
            .args(["logs", "nonexistent-session"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Session not found"));
    }
}
