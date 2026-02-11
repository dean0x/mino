//! List command - show active sessions

use crate::cli::args::{ListArgs, OutputFormat};
use crate::config::Config;
use crate::error::MinoResult;
use crate::session::{SessionManager, SessionStatus};
use crate::ui::{self, UiContext};
use console::style;

/// Execute the list command
pub async fn execute(args: ListArgs, _config: &Config) -> MinoResult<()> {
    let manager = SessionManager::new().await?;
    let sessions = manager.list().await?;

    let filtered: Vec<_> = if args.all {
        sessions
    } else {
        sessions
            .into_iter()
            .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Starting))
            .collect()
    };

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
        OutputFormat::Json => print_json(&filtered)?,
        OutputFormat::Plain => print_plain(&filtered),
    }

    Ok(())
}

fn print_table(sessions: &[crate::session::Session]) {
    let ctx = UiContext::detect();
    ui::intro(&ctx, "Sessions");

    println!(
        "{:<20} {:<12} {:<15} {:<30}",
        style("NAME").bold(),
        style("STATUS").bold(),
        style("STARTED").bold(),
        style("PROJECT").bold()
    );
    println!("{}", "-".repeat(77));

    for session in sessions {
        let status_styled = match session.status {
            SessionStatus::Running => style("running").green(),
            SessionStatus::Starting => style("starting").yellow(),
            SessionStatus::Stopped => style("stopped").dim(),
            SessionStatus::Failed => style("failed").red(),
        };

        let started = session.created_at.format("%Y-%m-%d %H:%M").to_string();
        let project = session
            .project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        println!(
            "{:<20} {:<12} {:<15} {:<30}",
            session.name, status_styled, started, project
        );
    }

    println!();
    println!("{} session(s)", sessions.len());
}

fn print_json(sessions: &[crate::session::Session]) -> MinoResult<()> {
    let json = serde_json::to_string_pretty(sessions)?;
    println!("{}", json);
    Ok(())
}

fn print_plain(sessions: &[crate::session::Session]) {
    for session in sessions {
        println!("{}", session.name);
    }
}
