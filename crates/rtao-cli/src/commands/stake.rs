// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/stake.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Familiar stake verbs using the generated transaction parser and executor.

use super::generated;
use crate::error::CliError;
use clap::{ArgMatches, Command};

const ALIASES: [(&str, &str); 7] = [
    ("add", "add_stake"),
    ("remove", "remove_stake"),
    ("move", "move_stake"),
    ("move-swap", "move_swap_stake"),
    ("transfer", "transfer_stake"),
    ("swap", "swap_stake"),
    ("burn", "stake_burn"),
];

/// Build the stake aliases whose real intent bodies are available in this binary.
pub fn command() -> Result<Command, CliError> {
    let available = rtao_intents::interface_metadata();
    let mut command = Command::new("stake")
        .about("Query and manage stake.")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for (alias, op) in ALIASES {
        if available
            .iter()
            .any(|row| row.name == op && row.compiled && row.implemented)
        {
            command = command.subcommand(generated::intent_command(op)?.name(alias));
        }
    }
    Ok(command)
}

/// Resolve a selected stake verb to its ordinary generated transaction operation.
pub fn operation(matches: &ArgMatches) -> Result<&'static str, CliError> {
    let (name, _) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing stake command"))?;
    ALIASES
        .iter()
        .find_map(|(alias, op)| (*alias == name).then_some(*op))
        .ok_or_else(|| CliError::input(format!("unknown stake command {name:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::command_tree;

    #[test]
    fn stake_aliases_keep_generated_arguments_and_hidden_compatibility_spellings()
    -> Result<(), Box<dyn std::error::Error>> {
        let address = rtao::primitives::AccountId([9; 32]).to_string();
        let cases = [
            (
                vec![
                    "rtao",
                    "st",
                    "add",
                    "--netuid",
                    "1",
                    "--hotkey",
                    &address,
                    "--amount",
                    "0.000000001",
                    "--json",
                ],
                "add_stake",
                "amount_tao",
            ),
            (
                vec![
                    "rtao",
                    "stake",
                    "move_swap",
                    "--origin-netuid",
                    "1",
                    "--dest-netuid",
                    "2",
                    "--origin-hotkey",
                    &address,
                    "--dest-hotkey",
                    &address,
                    "--amount-alpha",
                    "all",
                    "--dry-run",
                ],
                "move_swap_stake",
                "amount_alpha",
            ),
            (
                vec![
                    "rtao",
                    "stake",
                    "swap",
                    "--origin-netuid",
                    "0",
                    "--dest-netuid",
                    "2",
                    "--hotkey",
                    &address,
                    "--amount-alpha",
                    "1",
                    "--no-mev-shield",
                ],
                "swap_stake",
                "amount_alpha",
            ),
        ];
        for (argv, op, amount) in cases {
            let parsed = command_tree([command()?]).try_get_matches_from(argv)?;
            let (group, matches) = parsed.subcommand().ok_or("stake group")?;
            assert_eq!(group, "stake");
            assert_eq!(operation(matches)?, op);
            let leaf = matches.subcommand().ok_or("stake verb")?.1;
            assert!(leaf.get_one::<String>(amount).is_some());
            assert!(generated::intent_reserved(op)?.contains(&amount));
        }
        let mut command = command_tree([command()?]);
        let help = command.render_long_help().to_string();
        assert!(help.contains("stake"));
        assert!(!help.contains(" st "));
        Ok(())
    }
}
