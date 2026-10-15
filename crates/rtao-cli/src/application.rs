// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/main.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Extensible command-tree construction and hidden compatibility spellings.

use crate::globals::root_globals;
use clap::{Arg, Command};
use std::collections::BTreeSet;

/// Extend the CLI with implemented command families.
///
/// Callers supply real commands and retain their dispatch handlers; there are
/// no command placeholders. Aliases are registered recursively after extension,
/// so later generated `tx`/`query` commands inherit the same spelling rules.
pub fn command_tree(commands: impl IntoIterator<Item = Command>) -> Command {
    let root = root_globals(
        Command::new("rtao")
            .version(env!("CARGO_PKG_VERSION"))
            .about("Rust SDK and operator tooling for Bittensor subnets")
            .arg_required_else_help(true)
            .subcommand(
                Command::new("doctor")
                    .about("Check the workspace core pin, BLS patch and lockfile")
                    .arg(
                        Arg::new("manifest_path")
                            .long("manifest-path")
                            .value_name("Cargo.toml"),
                    ),
            )
            .subcommands(commands),
    );
    compatibility_aliases(root)
}

/// Register hidden snake_case aliases for every hyphenated command, plus the
/// v9 group aliases only for groups that actually exist in this tree.
pub fn compatibility_aliases(mut root: Command) -> Command {
    let mut occupied = root
        .get_subcommands()
        .flat_map(|child| std::iter::once(child.get_name()).chain(child.get_all_aliases()))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let children = root
        .get_subcommands()
        .map(|child| child.get_name().to_owned())
        .collect::<Vec<_>>();
    for name in children {
        root = root.mut_subcommand(&name, |child| {
            let mut child = compatibility_aliases(child);
            if name.contains('-') {
                let alias = name.replace('-', "_");
                if occupied.insert(alias.clone()) {
                    child = child.alias(alias);
                }
            }
            let aliases: &[&str] = match name.as_str() {
                "wallet" => &["w", "wallets"],
                "stake" => &["st"],
                "root" => &["rt"],
                "subnets" => &["s", "subnet"],
                "hparams" => &["su", "sudo"],
                "conviction" => &["lock"],
                "config" => &["c", "conf"],
                "addr" => &["addresses"],
                _ => &[],
            };
            for alias in aliases {
                if occupied.insert((*alias).to_owned()) {
                    child = child.alias(*alias);
                }
            }
            child
        });
    }
    // Old top-level aliases for groups moved under misc. No unsupported group
    // is exposed: aliases clone only registered, functional nested commands.
    if let Some(misc) = root.find_subcommand("misc") {
        let mut historical = Vec::new();
        for (name, aliases) in [
            ("weights", &["wt", "weight", "weights"][..]),
            ("crowd", &["cr", "crowdloan", "crowd"][..]),
            ("extension", &["extension"][..]),
            ("timelock", &["timelock"][..]),
            ("axon", &["axon"][..]),
            ("collateral", &["collateral"][..]),
            ("upgrade", &["upgrade"][..]),
        ] {
            if let Some(group) = misc.find_subcommand(name) {
                for alias in aliases {
                    historical.push(
                        group
                            .clone()
                            .name(*alias)
                            .alias(clap::builder::Resettable::<clap::builder::Str>::Reset)
                            .display_name(format!("misc {name}"))
                            .hide(true),
                    );
                }
            }
        }
        historical.push(
            misc.clone()
                .name("utils")
                .alias(clap::builder::Resettable::<clap::builder::Str>::Reset)
                .display_name("misc")
                .hide(true),
        );
        for alias in historical {
            if root.find_subcommand(alias.get_name()).is_none() {
                root = root.subcommand(alias);
            }
        }
    }
    root
}

/// Canonical command names for a parsed invocation, excluding the binary.
///
/// The original tree supplies the canonical display path attached to hidden
/// historical clones. A real top-level command keeps its own path, even if its
/// name happens to be an old alias such as `weights` or `utils`.
pub fn command_path(command: &Command, matches: &clap::ArgMatches) -> Vec<String> {
    let mut path = Vec::new();
    let mut current = matches;
    let mut selected = command;
    while let Some((name, child)) = current.subcommand() {
        let definition = selected.find_subcommand(name);
        let canonical = definition
            .filter(|command| command.is_hide_set())
            .and_then(Command::get_display_name)
            .filter(|name| *name == "misc" || name.starts_with("misc "));
        if let Some(canonical) = canonical {
            path.extend(canonical.split(' ').map(str::to_owned));
        } else {
            path.push(name.into());
        }
        if let Some(definition) = definition {
            selected = definition;
        }
        current = child;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_cover_future_generated_commands_without_help_noise()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = command_tree([
            Command::new("wallet").subcommand(Command::new("new-hotkey")),
            Command::new("tx").subcommand(Command::new("set-weights")),
            Command::new("misc")
                .subcommand(Command::new("weights").subcommand(Command::new("set"))),
        ]);
        for (argv, expected) in [
            (
                vec!["rtao", "w", "new_hotkey"],
                vec!["wallet", "new-hotkey"],
            ),
            (vec!["rtao", "tx", "set_weights"], vec!["tx", "set-weights"]),
            (vec!["rtao", "wt", "set"], vec!["misc", "weights", "set"]),
        ] {
            let parsed = root.clone().try_get_matches_from(argv)?;
            assert_eq!(command_path(&root, &parsed), expected);
        }
        let help = root.clone().render_long_help().to_string();
        assert!(!help.contains("new_hotkey"));
        assert!(!help.contains("wallets"));
        assert!(root.clone().try_get_matches_from(["rtao", "lock"]).is_err());
        Ok(())
    }

    #[test]
    fn cloned_misc_routes_and_real_top_level_commands_have_distinct_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = command_tree([
            Command::new("misc")
                .subcommand(Command::new("weights").subcommand(Command::new("set")))
                .subcommand(Command::new("crowd").subcommand(Command::new("list"))),
            Command::new("weights").subcommand(Command::new("inspect")),
            Command::new("utils").subcommand(Command::new("inspect")),
        ]);
        for (argv, expected) in [
            (vec!["rtao", "wt", "set"], vec!["misc", "weights", "set"]),
            (
                vec!["rtao", "weight", "set"],
                vec!["misc", "weights", "set"],
            ),
            (
                vec!["rtao", "crowdloan", "list"],
                vec!["misc", "crowd", "list"],
            ),
            (
                vec!["rtao", "weights", "inspect"],
                vec!["weights", "inspect"],
            ),
            (vec!["rtao", "utils", "inspect"], vec!["utils", "inspect"]),
        ] {
            let parsed = root.clone().try_get_matches_from(argv)?;
            assert_eq!(command_path(&root, &parsed), expected);
        }
        let root = command_tree([Command::new("misc")
            .subcommand(Command::new("weights").subcommand(Command::new("set")))]);
        let parsed = root
            .clone()
            .try_get_matches_from(["rtao", "utils", "weights", "set"])?;
        assert_eq!(command_path(&root, &parsed), ["misc", "weights", "set"]);
        Ok(())
    }

    #[test]
    fn automatic_aliases_preserve_registered_sibling_names_and_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = command_tree([
            Command::new("wallet").subcommand(Command::new("list")),
            Command::new("w").subcommand(Command::new("inspect")),
            Command::new("wallets").subcommand(Command::new("inspect")),
            Command::new("new-thing").subcommand(Command::new("inspect")),
            Command::new("new_thing").subcommand(Command::new("read")),
        ]);
        for (argv, expected) in [
            (vec!["rtao", "wallet", "list"], vec!["wallet", "list"]),
            (vec!["rtao", "w", "inspect"], vec!["w", "inspect"]),
            (
                vec!["rtao", "wallets", "inspect"],
                vec!["wallets", "inspect"],
            ),
            (
                vec!["rtao", "new-thing", "inspect"],
                vec!["new-thing", "inspect"],
            ),
            (vec!["rtao", "new_thing", "read"], vec!["new_thing", "read"]),
        ] {
            let parsed = root.clone().try_get_matches_from(argv)?;
            assert_eq!(command_path(&root, &parsed), expected);
        }
        let root = command_tree([
            Command::new("wallet").subcommand(Command::new("list")),
            Command::new("inspect").alias("w"),
        ]);
        for (argv, expected) in [
            (vec!["rtao", "w"], vec!["inspect"]),
            (vec!["rtao", "wallets", "list"], vec!["wallet", "list"]),
        ] {
            let parsed = root.clone().try_get_matches_from(argv)?;
            assert_eq!(command_path(&root, &parsed), expected);
        }
        Ok(())
    }

    #[test]
    fn historical_clones_clear_group_aliases_and_keep_nested_routes_functional()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = command_tree([
            Command::new("misc").alias("misc-alias").subcommand(
                Command::new("weights")
                    .alias("old_weights")
                    .subcommand(Command::new("set")),
            ),
            Command::new("old_weights").subcommand(Command::new("inspect")),
        ]);
        for (argv, expected) in [
            (vec!["rtao", "wt", "set"], vec!["misc", "weights", "set"]),
            (
                vec!["rtao", "weight", "set"],
                vec!["misc", "weights", "set"],
            ),
            (
                vec!["rtao", "weights", "set"],
                vec!["misc", "weights", "set"],
            ),
            (
                vec!["rtao", "utils", "old_weights", "set"],
                vec!["misc", "weights", "set"],
            ),
            (
                vec!["rtao", "misc-alias", "old_weights", "set"],
                vec!["misc", "weights", "set"],
            ),
            (
                vec!["rtao", "old_weights", "inspect"],
                vec!["old_weights", "inspect"],
            ),
        ] {
            let parsed = root.clone().try_get_matches_from(argv)?;
            assert_eq!(command_path(&root, &parsed), expected);
        }
        for clone in ["wt", "weight", "weights", "utils"] {
            assert_eq!(
                root.find_subcommand(clone)
                    .ok_or("clone")?
                    .get_all_aliases()
                    .count(),
                0
            );
        }
        Ok(())
    }
}

/// Parsed invocation after the generic required-input prompt round.
pub struct Invocation {
    /// Strictly validated clap arguments, including prompted public answers.
    pub matches: clap::ArgMatches,
    /// Actual public argv, ready for command dispatch and replay formatting.
    pub argv: Vec<std::ffi::OsString>,
    /// Invocation-scoped public prompt history.
    pub history: crate::prompt::PromptHistory,
}

/// Parse missing required values interactively using the command's actual clap
/// value parsers. Unknown options and invalid supplied values retain clap's
/// original error; JSON/quiet/piped sessions report missing inputs with exit 2.
///
/// Required option groups with alternative members remain clap-owned, because
/// choosing one on the user's behalf would change the command's meaning.
pub fn parse_with_prompts(
    command: Command,
    argv: Vec<std::ffi::OsString>,
    context: &crate::prompt::PromptContext,
    terminal: &mut dyn crate::prompt::Terminal,
) -> Result<Invocation, crate::error::CliError> {
    use crate::prompt::{
        PromptHistory, PromptResolver, PromptSpec, PromptValidation, fill_missing,
    };
    match command.clone().try_get_matches_from(&argv) {
        Ok(matches) => {
            return Ok(Invocation {
                matches,
                argv,
                history: PromptHistory::default(),
            });
        }
        Err(error) if error.kind() != clap::error::ErrorKind::MissingRequiredArgument => {
            return Err(clap_error(error));
        }
        Err(_) => {}
    }
    fn relax(mut command: Command) -> Command {
        let groups = command
            .get_groups()
            .map(|group| group.get_id().as_str().to_owned())
            .collect::<Vec<_>>();
        command = command
            .mut_args(|arg| {
                arg.required(false)
                    .requires(clap::builder::Resettable::<clap::Id>::Reset)
                    .required_unless_present(clap::builder::Resettable::<clap::Id>::Reset)
            })
            .mut_subcommands(relax);
        for group in groups {
            command = command.mut_group(group, |group| group.required(false));
        }
        command
    }
    let relaxed = relax(command.clone());
    let parsed = relaxed
        .clone()
        .try_get_matches_from(&argv)
        .map_err(clap_error)?;
    let mut required = Vec::new();
    let mut parsers = Vec::new();
    let mut selected_command = &command;
    let mut selected_matches = &parsed;
    let context = effective_prompt_context(context, &parsed);
    loop {
        for arg in selected_command
            .get_arguments()
            .filter(|arg| arg.is_required_set())
        {
            if selected_matches
                .value_source(arg.get_id().as_str())
                .is_some()
            {
                continue;
            }
            let mut spec = PromptSpec::new(
                arg.get_id().as_str(),
                arg.get_long()
                    .map(|value| format!("--{value}"))
                    .or_else(|| arg.get_short().map(|value| format!("-{value}")))
                    .unwrap_or_else(|| arg.get_id().as_str().into()),
            );
            spec.positional = arg.is_positional();
            spec.help = arg.get_help().map(ToString::to_string);
            // Validate the actual value type in isolation. Relationships such
            // as a.requires(b) belong to the final complete invocation and
            // must not reject a while b is still waiting for its prompt.
            let mut value = Arg::new(arg.get_id().clone())
                .value_parser(arg.get_value_parser().clone())
                .action(arg.get_action().clone())
                .allow_hyphen_values(true)
                .ignore_case(arg.is_ignore_case_set());
            if let Some(range) = arg.get_num_args() {
                value = value.num_args(range);
            }
            if let Some(delimiter) = arg.get_value_delimiter() {
                value = value.value_delimiter(delimiter);
            }
            parsers.push((spec.field.clone(), Command::new("answer").arg(value)));
            required.push(spec);
        }
        if let Some((name, child)) = selected_matches.subcommand() {
            selected_command = selected_command
                .find_subcommand(name)
                .ok_or_else(|| crate::error::CliError::input("unknown command path"))?;
            selected_matches = child;
        } else {
            break;
        }
    }
    if required.is_empty() {
        return Err(clap_error(
            command
                .clone()
                .try_get_matches_from(&argv)
                .err()
                .ok_or_else(|| {
                    crate::error::CliError::input("required argument validation changed")
                })?,
        ));
    }
    struct Parser {
        parsers: Vec<(String, Command)>,
        argv: Vec<std::ffi::OsString>,
    }
    impl PromptResolver for Parser {
        fn parse(
            &mut self,
            spec: &PromptSpec,
            raw: &str,
        ) -> Result<serde_json::Value, PromptValidation> {
            let command = &self
                .parsers
                .iter()
                .find(|(field, _)| field == &spec.field)
                .ok_or_else(|| PromptValidation::Message("missing value parser".into()))?
                .1;
            command
                .clone()
                .try_get_matches_from(["answer", "--", raw])
                .map_err(|error| PromptValidation::Message(error.to_string()))?;
            if !spec.positional {
                self.argv.push(spec.flag.clone().into());
            }
            self.argv.push(raw.into());
            Ok(serde_json::Value::String(raw.into()))
        }
    }
    let mut parser = Parser {
        parsers,
        argv: argv.clone(),
    };
    let mut history = PromptHistory::default();
    fill_missing(
        &context,
        terminal,
        &required,
        &mut serde_json::Map::new(),
        &mut parser,
        &mut history,
    )
    .map_err(|error| crate::error::CliError {
        code: error.exit_code(),
        message: error.to_string(),
        help: None,
    })?;
    let matches = command
        .try_get_matches_from(&parser.argv)
        .map_err(clap_error)?;
    Ok(Invocation {
        matches,
        argv: parser.argv,
        history,
    })
}

fn effective_prompt_context(
    context: &crate::prompt::PromptContext,
    parsed: &clap::ArgMatches,
) -> crate::prompt::PromptContext {
    let mut context = *context;
    let mut current = parsed;
    loop {
        let enabled = |id: &str| {
            current
                .try_get_one::<bool>(id)
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false)
        };
        context.json |= enabled("json");
        context.quiet |= enabled("quiet");
        context.assume_yes |= enabled("assume_yes");
        let backend = current
            .try_get_one::<String>("signer_backend")
            .ok()
            .flatten()
            .filter(|backend| !backend.is_empty());
        if enabled("ledger") {
            context.external_signer = true;
            context.extension_signer = false;
        } else if let Some(backend) = backend {
            let backend = backend.trim().to_ascii_lowercase();
            context.external_signer = ["ledger", "vault", "extension"].contains(&backend.as_str());
            context.extension_signer = backend == "extension";
        }
        match current.subcommand() {
            Some((_, child)) => current = child,
            None => break,
        }
    }
    context
}

fn clap_error(error: clap::Error) -> crate::error::CliError {
    crate::error::CliError {
        code: if error.use_stderr() { 2 } else { 0 },
        message: error.to_string(),
        help: None,
    }
}

#[cfg(test)]
mod parsing_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::prompt::{PromptContext, PromptError, Terminal};
    use std::collections::VecDeque;
    struct Input {
        tty: bool,
        replies: VecDeque<String>,
        read: usize,
        messages: Vec<String>,
    }
    impl Terminal for Input {
        fn stdin_is_tty(&self) -> bool {
            self.tty
        }
        fn stderr_is_tty(&self) -> bool {
            self.tty
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            self.read += 1;
            self.replies.pop_front().ok_or(PromptError::Aborted)
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, text: &str) -> Result<(), PromptError> {
            self.messages.push(text.into());
            Ok(())
        }
    }
    fn command() -> Command {
        command_tree([Command::new("choose").arg(
            Arg::new("netuid")
                .long("netuid")
                .required(true)
                .value_parser(clap::value_parser!(u16)),
        )])
    }

    fn input(replies: &[&str]) -> Input {
        Input {
            tty: true,
            replies: replies.iter().map(|reply| (*reply).into()).collect(),
            read: 0,
            messages: Vec::new(),
        }
    }

    fn dependent_command() -> Command {
        command_tree([Command::new("choose")
            .arg(
                Arg::new("a")
                    .long("a")
                    .required(true)
                    .requires("b")
                    .value_parser(clap::value_parser!(u16)),
            )
            .arg(
                Arg::new("b")
                    .long("b")
                    .required(true)
                    .value_parser(clap::value_parser!(u16)),
            )])
    }

    #[test]
    fn dependent_required_values_validate_individually_then_as_a_complete_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut terminal = input(&["65536", "12", "34"]);
        let invocation = parse_with_prompts(
            dependent_command(),
            vec!["rtao".into(), "choose".into()],
            &PromptContext::default(),
            &mut terminal,
        )?;
        assert_eq!(terminal.read, 3);
        let leaf = invocation.matches.subcommand().ok_or("leaf")?.1;
        assert_eq!(leaf.get_one::<u16>("a"), Some(&12));
        assert_eq!(leaf.get_one::<u16>("b"), Some(&34));
        assert_eq!(
            invocation
                .history
                .replay_command(&["rtao".into(), "choose".into()]),
            "rtao choose --a 12 --b 34"
        );
        let mut terminal = input(&["34"]);
        let invocation = parse_with_prompts(
            dependent_command(),
            ["rtao", "choose", "--a", "12"]
                .into_iter()
                .map(Into::into)
                .collect(),
            &PromptContext::default(),
            &mut terminal,
        )?;
        assert_eq!(terminal.read, 1);
        assert_eq!(
            invocation
                .matches
                .subcommand()
                .ok_or("leaf")?
                .1
                .get_one::<u16>("b"),
            Some(&34)
        );
        Ok(())
    }

    #[test]
    fn supplied_errors_and_alternative_groups_remain_clap_owned()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut terminal = input(&[]);
        for argv in [
            vec!["rtao", "choose", "--a", "invalid"],
            vec!["rtao", "choose", "--a", "12", "--unknown"],
        ] {
            let error = parse_with_prompts(
                dependent_command(),
                argv.into_iter().map(Into::into).collect(),
                &PromptContext::default(),
                &mut terminal,
            )
            .err()
            .ok_or("error")?;
            assert_eq!(error.code, 2);
        }
        let group = command_tree([Command::new("choose")
            .arg(Arg::new("a").long("a"))
            .arg(Arg::new("b").long("b"))
            .group(
                clap::ArgGroup::new("choice")
                    .args(["a", "b"])
                    .required(true),
            )]);
        let error = parse_with_prompts(
            group,
            vec!["rtao".into(), "choose".into()],
            &PromptContext::default(),
            &mut terminal,
        )
        .err()
        .ok_or("group error")?;
        assert_eq!(error.code, 2);
        assert!(error.message.contains("--a") && error.message.contains("--b"));
        let conditional = command_tree([Command::new("choose")
            .arg(Arg::new("a").long("a").requires("b"))
            .arg(Arg::new("b").long("b"))]);
        let error = parse_with_prompts(
            conditional,
            ["rtao", "choose", "--a", "supplied"]
                .into_iter()
                .map(Into::into)
                .collect(),
            &PromptContext::default(),
            &mut terminal,
        )
        .err()
        .ok_or("conditional error")?;
        assert_eq!(error.code, 2);
        assert!(error.message.contains("--b"));
        assert_eq!(terminal.read, 0);
        Ok(())
    }

    #[test]
    fn short_only_required_value_uses_its_real_flag_and_parser()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = command_tree([Command::new("choose").arg(
            Arg::new("index")
                .short('x')
                .required(true)
                .value_parser(clap::value_parser!(u16)),
        )]);
        let mut terminal = input(&["bad", "7"]);
        let invocation = parse_with_prompts(
            command,
            vec!["rtao".into(), "choose".into()],
            &PromptContext::default(),
            &mut terminal,
        )?;
        assert_eq!(terminal.read, 2);
        assert_eq!(
            invocation
                .matches
                .subcommand()
                .ok_or("leaf")?
                .1
                .get_one::<u16>("index"),
            Some(&7)
        );
        assert_eq!(
            invocation
                .history
                .replay_command(&["rtao".into(), "choose".into()]),
            "rtao choose -x 7"
        );
        Ok(())
    }

    #[test]
    fn root_and_leaf_json_quiet_and_explicit_noninteractive_modes_never_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = crate::globals::with_globals(
            Command::new("choose").arg(
                Arg::new("netuid")
                    .long("netuid")
                    .required(true)
                    .value_parser(clap::value_parser!(u16)),
            ),
            crate::globals::Tier::Tx,
        );
        for argv in [
            vec!["rtao", "--json", "choose"],
            vec!["rtao", "choose", "--json"],
            vec!["rtao", "-q", "choose"],
            vec!["rtao", "choose", "-q"],
            vec!["rtao", "choose", "--quiet", "--yes"],
            vec!["rtao", "choose", "--json", "--signer", "extension"],
        ] {
            let mut terminal = input(&[]);
            let error = parse_with_prompts(
                command_tree([command.clone()]),
                argv.into_iter().map(Into::into).collect(),
                &PromptContext::default(),
                &mut terminal,
            )
            .err()
            .ok_or("noninteractive error")?;
            assert_eq!(error.code, 2);
            assert!(error.message.contains("--netuid"));
            assert_eq!(terminal.read, 0);
        }
        let mut terminal = input(&[]);
        let context = PromptContext {
            policy: rtao::wallet::PromptPolicy::Never,
            ..Default::default()
        };
        let error = parse_with_prompts(
            command_tree([command]),
            vec!["rtao".into(), "choose".into()],
            &context,
            &mut terminal,
        )
        .err()
        .ok_or("Never error")?;
        assert_eq!(error.code, 2);
        assert_eq!(terminal.read, 0);
        Ok(())
    }

    #[test]
    fn yes_and_external_modes_skip_confirmations_but_allow_required_values_as_python_does()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = command_tree([crate::globals::with_globals(
            Command::new("choose").arg(
                Arg::new("netuid")
                    .long("netuid")
                    .required(true)
                    .value_parser(clap::value_parser!(u16)),
            ),
            crate::globals::Tier::Tx,
        )]);
        let mut terminal = input(&["12"]);
        let invocation = parse_with_prompts(
            command.clone(),
            [
                "rtao",
                "--signer",
                "extension",
                "choose",
                "--yes",
                "--signer",
                "wallet",
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
            &PromptContext::default(),
            &mut terminal,
        )?;
        assert_eq!(terminal.read, 1);
        let context = effective_prompt_context(&PromptContext::default(), &invocation.matches);
        assert!(context.assume_yes);
        assert!(!context.external_signer && !context.extension_signer);
        let parsed = command.clone().try_get_matches_from([
            "rtao",
            "--yes",
            "choose",
            "--netuid",
            "12",
            "--signer",
            "extension",
        ])?;
        let context = effective_prompt_context(&PromptContext::default(), &parsed);
        assert!(context.assume_yes && context.external_signer && context.extension_signer);
        let parsed = command.try_get_matches_from([
            "rtao",
            "choose",
            "--netuid",
            "12",
            "--signer",
            "extension",
            "--ledger",
        ])?;
        let context = effective_prompt_context(&PromptContext::default(), &parsed);
        assert!(context.external_signer && !context.extension_signer);
        Ok(())
    }

    #[test]
    fn final_original_parser_rejects_conflicts_after_all_values_are_collected()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = command_tree([Command::new("choose")
            .arg(Arg::new("a").long("a").required(true).conflicts_with("b"))
            .arg(Arg::new("b").long("b").required(true))]);
        let mut terminal = input(&["first", "second"]);
        let error = parse_with_prompts(
            command,
            vec!["rtao".into(), "choose".into()],
            &PromptContext::default(),
            &mut terminal,
        )
        .err()
        .ok_or("strict conflict")?;
        assert_eq!(terminal.read, 2);
        assert_eq!(error.code, 2);
        assert!(error.message.contains("cannot be used with"));
        Ok(())
    }
    #[test]
    fn actual_clap_parser_reasks_missing_invalid_value_and_refuses_pipes()
    -> Result<(), Box<dyn std::error::Error>> {
        let argv = vec!["rtao".into(), "choose".into()];
        let mut input = Input {
            tty: true,
            replies: VecDeque::from(["65536".into(), "12".into()]),
            read: 0,
            messages: Vec::new(),
        };
        let invocation = parse_with_prompts(
            command(),
            argv.clone(),
            &PromptContext::default(),
            &mut input,
        )?;
        assert_eq!(input.read, 2);
        assert_eq!(
            invocation
                .matches
                .subcommand()
                .unwrap()
                .1
                .get_one::<u16>("netuid"),
            Some(&12)
        );
        assert_eq!(
            invocation
                .history
                .replay_command(&["rtao".into(), "choose".into()]),
            "rtao choose --netuid 12"
        );
        input.tty = false;
        let error = parse_with_prompts(command(), argv, &PromptContext::default(), &mut input)
            .err()
            .ok_or("piped missing refusal")?;
        assert_eq!(error.code, 2);
        assert_eq!(input.read, 2);
        Ok(())
    }
    #[test]
    fn invalid_supplied_and_unknown_inputs_do_not_prompt() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut input = Input {
            tty: true,
            replies: VecDeque::new(),
            read: 0,
            messages: Vec::new(),
        };
        for argv in [
            vec!["rtao", "choose", "--netuid", "invalid"],
            vec!["rtao", "choose", "--unknown"],
        ] {
            let error = parse_with_prompts(
                command(),
                argv.into_iter().map(Into::into).collect(),
                &PromptContext::default(),
                &mut input,
            )
            .err()
            .ok_or("reject")?;
            assert_eq!(error.code, 2);
        }
        assert_eq!(input.read, 0);
        Ok(())
    }
}
