//! Shell completion scripts generated from the installed command tree.

use clap::{Arg, ArgMatches, Command, value_parser};
use clap_complete::{Shell, generate};

use crate::{
    error::CliError,
    globals::{Tier, with_globals},
    output::{Output, Stream},
};

/// Build the offline shell-completion command.
pub fn command() -> Command {
    with_globals(
        Command::new("completions")
            .about("Print a shell completion script for rtao.")
            .long_about(
                "Print a shell completion script for the commands available in this build. \
             Save the script in your shell's completion directory or source it from \
             your shell configuration. This command writes shell source to stdout.",
            ),
        Tier::Read,
    )
    .arg(
        Arg::new("shell")
            .value_parser(value_parser!(Shell))
            .required(true)
            .help("Shell that will load the generated script."),
    )
}

/// Generate shell source from the final application tree and write it once.
///
/// The caller supplies the same tree it parses, including optional features and
/// compatibility spellings. Generation has no network or filesystem effects.
/// Shell source is primary output, including with `--quiet` or `--json`.
pub fn run(tree: &Command, matches: &ArgMatches, output: &Output) -> Result<(), CliError> {
    let shell = matches
        .try_get_one::<Shell>("shell")
        .map_err(|error| CliError::input(error.to_string()))?
        .copied()
        .ok_or_else(|| CliError::input("missing completion shell"))?;
    let mut tree = tree.clone();
    let mut script = Vec::new();
    generate(shell, &mut tree, "rtao", &mut script);
    output
        .sink()
        .write(Stream::Stdout, &script)
        .map_err(|error| CliError::failure(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        application,
        output::{MemorySink, OutputOptions},
    };

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn tree() -> Command {
        application::command_tree([
            command(),
            Command::new("wallet").subcommand(
                Command::new("new-hotkey").arg(Arg::new("name").long("name").help("Hotkey name")),
            ),
            Command::new("misc").subcommand(
                Command::new("timelock")
                    .subcommand(Command::new("encrypt").arg(Arg::new("round").long("round"))),
            ),
        ])
    }

    #[test]
    fn shells_use_the_supplied_nested_tree_and_do_not_emit_diagnostics() -> TestResult {
        for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
            let tree = tree();
            let matches = tree.clone().try_get_matches_from([
                "rtao",
                "completions",
                shell,
                "--quiet",
                "--json",
            ])?;
            let (_, matches) = matches.subcommand().ok_or("completion invocation")?;
            let sink = Arc::new(MemorySink::default());
            let output = Output::new(
                OutputOptions {
                    json_mode: true,
                    quiet: true,
                    network: None,
                },
                sink.clone(),
            );
            run(&tree, matches, &output)?;
            let script = sink.text(Stream::Stdout)?;
            for expected in [
                "rtao",
                "wallet",
                "new-hotkey",
                "timelock",
                "encrypt",
                "round",
            ] {
                assert!(script.contains(expected), "{shell} omitted {expected}");
            }
            assert!(sink.bytes(Stream::Stderr)?.is_empty());
        }
        Ok(())
    }

    #[test]
    fn optional_commands_are_completed_only_when_supplied_by_the_caller() -> TestResult {
        let base = tree();
        for enabled in [false, true] {
            let tree = if enabled {
                base.clone()
                    .subcommand(Command::new("feature-specific-fixture"))
            } else {
                base.clone()
            };
            let matches = tree
                .clone()
                .try_get_matches_from(["rtao", "completions", "bash"])?;
            let (_, matches) = matches.subcommand().ok_or("completion invocation")?;
            let sink = Arc::new(MemorySink::default());
            let output = Output::new(OutputOptions::default(), sink.clone());
            run(&tree, matches, &output)?;
            assert_eq!(
                sink.text(Stream::Stdout)?
                    .contains("feature-specific-fixture"),
                enabled
            );
        }
        Ok(())
    }

    struct BrokenSink;

    impl crate::output::OutputSink for BrokenSink {
        fn write(&self, _: Stream, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed completion consumer",
            ))
        }
    }

    #[test]
    fn broken_pipe_is_reported_and_unknown_shell_is_rejected() -> TestResult {
        let tree = tree();
        assert!(
            tree.clone()
                .try_get_matches_from(["rtao", "completions", "unknown-shell"])
                .is_err()
        );
        let matches = tree
            .clone()
            .try_get_matches_from(["rtao", "completions", "bash"])?;
        let (_, matches) = matches.subcommand().ok_or("completion invocation")?;
        let output = Output::new(OutputOptions::default(), Arc::new(BrokenSink));
        assert!(run(&tree, matches, &output).is_err());
        Ok(())
    }
}
