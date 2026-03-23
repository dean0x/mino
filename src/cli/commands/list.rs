//! List command - show active sessions

use crate::cli::args::{ListArgs, OutputFormat};
use crate::config::Config;
use crate::error::MinoResult;
use crate::session::{Session, SessionManager, SessionStatus};
use crate::ui::{self, UiContext};
use console::{pad_str, Alignment, style};

/// Execute the list command
pub async fn execute(args: ListArgs, _config: &Config) -> MinoResult<()> {
    let manager = SessionManager::new().await?;
    let sessions = manager.list().await?;

    let filtered = filter_sessions(sessions, args.all);

    if filtered.is_empty() {
        match args.format {
            OutputFormat::Json => println!("[]"),
            OutputFormat::Plain => {}
            OutputFormat::Table => {
                let ctx = UiContext::detect();
                ui::step_info(&ctx, "No active sessions");
            }
        }
        return Ok(());
    }

    match args.format {
        OutputFormat::Table => print_table(&filtered),
        OutputFormat::Json => {
            let json = format_json(&filtered)?;
            println!("{}", json);
        }
        OutputFormat::Plain => {
            let plain = format_plain(&filtered);
            print!("{}", plain);
        }
    }

    Ok(())
}

/// Filter sessions by active status (Running/Starting) unless `show_all` is true.
fn filter_sessions(sessions: Vec<Session>, show_all: bool) -> Vec<Session> {
    if show_all {
        sessions
    } else {
        sessions
            .into_iter()
            .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Starting))
            .collect()
    }
}

/// Format sessions as pretty-printed JSON.
fn format_json(sessions: &[Session]) -> MinoResult<String> {
    Ok(serde_json::to_string_pretty(sessions)?)
}

/// Format sessions as plain text, one name per line.
fn format_plain(sessions: &[Session]) -> String {
    sessions.iter().map(|s| format!("{}\n", s.name)).collect()
}

fn print_table(sessions: &[Session]) {
    const W_NAME: usize = 20;
    const W_STATUS: usize = 12;
    const W_STARTED: usize = 15;
    const W_PROJECT: usize = 30;

    let ctx = UiContext::detect();
    ui::intro(&ctx, "Sessions");

    println!(
        "{} {} {} {}",
        pad_str(&style("NAME").bold().to_string(), W_NAME, Alignment::Left, None),
        pad_str(&style("STATUS").bold().to_string(), W_STATUS, Alignment::Left, None),
        pad_str(&style("STARTED").bold().to_string(), W_STARTED, Alignment::Left, None),
        pad_str(&style("PROJECT").bold().to_string(), W_PROJECT, Alignment::Left, None),
    );
    println!("{}", "-".repeat(W_NAME + 1 + W_STATUS + 1 + W_STARTED + 1 + W_PROJECT));

    for session in sessions {
        let status_styled = match session.status {
            SessionStatus::Running => style("running").green().to_string(),
            SessionStatus::Starting => style("starting").yellow().to_string(),
            SessionStatus::Stopped => style("stopped").dim().to_string(),
            SessionStatus::Failed => style("failed").red().to_string(),
        };

        let started = session.created_at.format("%Y-%m-%d %H:%M").to_string();
        let project = session
            .project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        println!(
            "{} {} {} {}",
            pad_str(&session.name, W_NAME, Alignment::Left, None),
            pad_str(&status_styled, W_STATUS, Alignment::Left, None),
            pad_str(&started, W_STARTED, Alignment::Left, None),
            pad_str(project, W_PROJECT, Alignment::Left, None),
        );
    }

    println!();
    println!("{} session(s)", sessions.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::mock::test_session;

    #[test]
    fn filter_active_only() {
        let sessions = vec![
            test_session("running-1", SessionStatus::Running, Some("c1")),
            test_session("stopped-1", SessionStatus::Stopped, Some("c2")),
            test_session("starting-1", SessionStatus::Starting, Some("c3")),
            test_session("failed-1", SessionStatus::Failed, Some("c4")),
        ];

        let filtered = filter_sessions(sessions, false);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "running-1");
        assert_eq!(filtered[1].name, "starting-1");
    }

    #[test]
    fn filter_all_returns_everything() {
        let sessions = vec![
            test_session("running-1", SessionStatus::Running, Some("c1")),
            test_session("stopped-1", SessionStatus::Stopped, Some("c2")),
            test_session("failed-1", SessionStatus::Failed, Some("c3")),
        ];

        let filtered = filter_sessions(sessions, true);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn filter_empty_input() {
        let filtered = filter_sessions(vec![], false);
        assert!(filtered.is_empty());

        let filtered = filter_sessions(vec![], true);
        assert!(filtered.is_empty());
    }

    #[test]
    fn json_output_valid() {
        let sessions = vec![test_session(
            "my-session",
            SessionStatus::Running,
            Some("c1"),
        )];

        let json = format_json(&sessions).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "my-session");
        assert_eq!(arr[0]["status"], "running");
    }

    #[test]
    fn plain_output_names_only() {
        let sessions = vec![
            test_session("session-a", SessionStatus::Running, Some("c1")),
            test_session("session-b", SessionStatus::Stopped, Some("c2")),
        ];

        let plain = format_plain(&sessions);
        let lines: Vec<&str> = plain.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "session-a");
        assert_eq!(lines[1], "session-b");
    }
}
