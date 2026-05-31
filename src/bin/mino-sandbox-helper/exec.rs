use std::collections::HashMap;
use std::path::PathBuf;

use mino::sandbox::helper;
use mino::session::validate_session_name;

/// Parsed arguments for the exec subcommand.
#[derive(Debug)]
pub(crate) struct ExecArgs<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) sandbox_user: &'a str,
    pub(crate) command: &'a [String],
}

/// Parse exec subcommand arguments into an ExecArgs struct.
pub(crate) fn parse_exec_args(args: &[String]) -> Result<ExecArgs<'_>, String> {
    let mut session_id: Option<&str> = None;
    let mut sandbox_user: Option<&str> = None;
    let mut command_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session-id" => {
                session_id = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--sandbox-user" => {
                sandbox_user = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--pid" => {
                i += 2; // Accepted for compat, not used for exec
            }
            "--" => {
                command_start = Some(i + 1);
                break;
            }
            _ => {
                i += 1;
            }
        }
    }

    let session_id = match session_id {
        Some(id) if !id.is_empty() => id,
        _ => return Err("Missing --session-id argument".to_string()),
    };

    let sandbox_user = match sandbox_user {
        Some(u) if !u.is_empty() => u,
        _ => return Err("Missing --sandbox-user argument".to_string()),
    };

    let command: &[String] = match command_start {
        Some(idx) if idx < args.len() => &args[idx..],
        _ => return Err("Missing command after '--'".to_string()),
    };

    if command.is_empty() {
        return Err("Empty command".to_string());
    }

    Ok(ExecArgs {
        session_id,
        sandbox_user,
        command,
    })
}

/// Execute a command, optionally with a custom environment.
///
/// With `Some(env)`: clears the process environment and sets only the provided vars.
/// With `None`: inherits the current environment.
///
/// On success, this function never returns (the process image is replaced).
/// On failure, returns the IO error from the exec attempt.
#[cfg(unix)]
pub(crate) fn exec_command(
    command: &[String],
    env: Option<&HashMap<String, String>>,
) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    if let Some(env_map) = env {
        cmd.env_clear();
        cmd.envs(env_map);
    }
    cmd.exec()
}

/// Execute a command as the sandbox user inside an existing session.
///
/// Unlike `spawn`, this does not set up ACLs or fork — it simply drops
/// privileges to the sandbox user and execs the command directly.
/// ACLs from the original `spawn` are still active on the session's paths.
///
/// Usage: mino-sandbox-helper exec --session-id <id> --sandbox-user <user> [--pid <pid>] -- <command...>
pub(crate) fn handle_exec(args: &[String]) -> Result<i32, String> {
    let parsed = parse_exec_args(args)?;

    // Validate session_id using the library function
    validate_session_name(parsed.session_id).map_err(|e| format!("Invalid session_id: {}", e))?;

    mino::sandbox::config::validate_sandbox_user(parsed.sandbox_user)
        .map_err(|e| format!("Invalid sandbox_user: {}", e))?;

    eprintln!(
        "[mino-helper] exec session={} command={:?}",
        parsed.session_id, &parsed.command[0]
    );

    let (uid, gid) = super::lifecycle::get_user_ids(parsed.sandbox_user)?;

    // SAFETY: drop_privileges calls setgroups/setgid/setuid — must run as root.
    #[cfg(unix)]
    unsafe {
        super::lifecycle::drop_privileges(uid, gid)?;
    }

    #[cfg(not(unix))]
    {
        let _ = (uid, gid);
        return Err("Exec is only supported on Unix".to_string());
    }

    // Build minimal env for exec (don't inherit root's environment)
    let home_dir = PathBuf::from(format!("/tmp/mino-home-{}", parsed.session_id));
    let exec_env = helper::build_exec_env(&home_dir, parsed.sandbox_user)
        .map_err(|e| format!("failed to build exec env: {}", e))?;

    // exec the command — this replaces the current process
    let err = exec_command(parsed.command, Some(&exec_env));
    Err(format!("exec failed: {}", err))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::str_args as args;

    #[test]
    fn parse_exec_args_valid() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "--",
            "bash",
            "-c",
            "echo hello",
        ]);
        let parsed = parse_exec_args(&input).unwrap();
        assert_eq!(parsed.session_id, "my-session");
        assert_eq!(parsed.sandbox_user, "_mino_agent");
        assert_eq!(parsed.command.len(), 3);
        assert_eq!(parsed.command[0], "bash");
        assert_eq!(parsed.command[1], "-c");
        assert_eq!(parsed.command[2], "echo hello");
    }

    #[test]
    fn parse_exec_args_missing_session_id() {
        let input = args(&["--sandbox-user", "_mino_agent", "--", "bash"]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("--session-id"),
            "expected error about --session-id, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_missing_sandbox_user() {
        let input = args(&["--session-id", "my-session", "--", "bash"]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("--sandbox-user"),
            "expected error about --sandbox-user, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_missing_separator() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "bash",
        ]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("command") || err.contains("--"),
            "expected error about missing command/separator, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_empty_command_after_separator() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "--",
        ]);
        let err = parse_exec_args(&input).unwrap_err();
        assert!(
            err.contains("command"),
            "expected error about missing/empty command, got: {}",
            err
        );
    }

    #[test]
    fn parse_exec_args_pid_flag_accepted_and_ignored() {
        let input = args(&[
            "--session-id",
            "my-session",
            "--sandbox-user",
            "_mino_agent",
            "--pid",
            "12345",
            "--",
            "ls",
        ]);
        let parsed = parse_exec_args(&input).unwrap();
        assert_eq!(parsed.session_id, "my-session");
        assert_eq!(parsed.sandbox_user, "_mino_agent");
        assert_eq!(parsed.command.len(), 1);
        assert_eq!(parsed.command[0], "ls");
    }
}
