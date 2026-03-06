//! Shell completion generation

use crate::cli::{Cli, CompletionsArgs};
use crate::error::MinoResult;
use clap::CommandFactory;
use clap_complete::generate;

/// Generate shell completions and write to stdout
pub async fn execute(args: CompletionsArgs) -> MinoResult<()> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_owned();
    generate(args.shell, &mut cmd, name, &mut std::io::stdout());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_complete::Shell;

    fn generate_completions(shell: Shell) -> String {
        let mut cmd = Cli::command();
        let name = cmd.get_name().to_owned();
        let mut buf = Vec::new();
        generate(shell, &mut cmd, &name, &mut buf);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn generates_bash_completions() {
        let output = generate_completions(Shell::Bash);
        assert!(output.contains("mino"));
        assert!(output.contains("run"));
        assert!(output.contains("completions"));
    }

    #[test]
    fn generates_zsh_completions() {
        let output = generate_completions(Shell::Zsh);
        assert!(output.contains("mino"));
        assert!(output.contains("run"));
    }

    #[test]
    fn generates_fish_completions() {
        let output = generate_completions(Shell::Fish);
        assert!(output.contains("mino"));
        assert!(output.contains("run"));
    }
}
