//! Compare the installed CLI with pinned Python registrations without Python.
//!
//! This source inventory proves command/option spellings, positional counts and
//! compatibility aliases. Click-resolved defaults, types and environment rules
//! have separate config/schema fixtures; this is not a Python oracle result.

use std::{collections::BTreeMap, fs, path::Path};

use clap::Command;
use rtao_cli::commands;
use serde::Deserialize;
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Deserialize)]
struct Inventory {
    schema: u32,
    provenance: String,
    sources: Vec<Source>,
    aliases: BTreeMap<String, String>,
    commands: Vec<Registration>,
}

#[derive(Deserialize)]
struct Source {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Registration {
    path: String,
    source: String,
    options: Vec<String>,
    positionals: Vec<String>,
    unresolved: Vec<String>,
    generated_intent: Option<String>,
}

fn inventory() -> Result<Inventory, serde_json::Error> {
    serde_json::from_str(include_str!("fixtures/cli_source_tree.json"))
}

fn enabled(path: &str) -> bool {
    (!path.starts_with("evm ") || cfg!(feature = "evm"))
        && (!path.starts_with("misc extension") || cfg!(feature = "extension"))
}

fn find<'a>(mut command: &'a Command, path: &str) -> Option<&'a Command> {
    for component in path.split(' ') {
        command = command.find_subcommand(component)?;
    }
    Some(command)
}

fn has_option(command: &Command, spelling: &str) -> bool {
    command.get_arguments().any(|argument| {
        if let Some(long) = spelling.strip_prefix("--") {
            argument.get_long() == Some(long)
                || argument
                    .get_all_aliases()
                    .is_some_and(|aliases| aliases.contains(&long))
        } else {
            let mut characters = spelling.trim_start_matches('-').chars();
            let short = characters.next();
            characters.next().is_none()
                && (argument.get_short() == short
                    || argument
                        .get_all_short_aliases()
                        .is_some_and(|aliases| short.is_some_and(|short| aliases.contains(&short))))
        }
    })
}

#[test]
fn pinned_source_inventory_has_current_hashes_and_no_unresolved_parameters() -> TestResult {
    let inventory = inventory()?;
    assert_eq!(inventory.schema, 1);
    assert!(
        inventory
            .provenance
            .contains("Python/Click was not executed")
    );
    assert_eq!(inventory.commands.len(), 165);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for source in inventory.sources {
        assert!(
            source
                .path
                .starts_with("vendor/upstream/src/sdk/python/bittensor/cli/")
        );
        assert_eq!(
            hex::encode(Sha256::digest(fs::read(root.join(&source.path))?)),
            source.sha256,
            "review and refresh the source inventory for {}",
            source.path
        );
    }
    for registration in inventory.commands {
        assert!(registration.unresolved.is_empty(), "{}", registration.path);
    }
    Ok(())
}

#[test]
fn every_enabled_source_registration_and_owned_spelling_exists_in_installed_tree() -> TestResult {
    let mut tree = commands::command_tree()?;
    tree.build();
    let mut failures = Vec::new();
    for registration in inventory()?.commands {
        if !enabled(&registration.path) {
            continue;
        }
        let Some(command) = find(&tree, &registration.path) else {
            failures.push(format!(
                "missing {} ({})",
                registration.path, registration.source
            ));
            continue;
        };
        for spelling in registration.options {
            if !has_option(command, &spelling) {
                failures.push(format!("{} lacks {spelling}", registration.path));
            }
        }
        if registration.generated_intent.is_none()
            && command.get_positionals().count() != registration.positionals.len()
        {
            failures.push(format!(
                "{} has {} positionals; Python declares {:?}",
                registration.path,
                command.get_positionals().count(),
                registration.positionals
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

fn same_surface(left: &Command, right: &Command, path: &str) {
    let mut left_args = left
        .get_arguments()
        .map(|arg| arg.get_id().as_str())
        .collect::<Vec<_>>();
    let mut right_args = right
        .get_arguments()
        .map(|arg| arg.get_id().as_str())
        .collect::<Vec<_>>();
    left_args.sort_unstable();
    right_args.sort_unstable();
    assert_eq!(left_args, right_args, "alias argument mismatch: {path}");
    for child in right.get_subcommands() {
        let alias_child = left.find_subcommand(child.get_name());
        assert!(
            alias_child.is_some(),
            "alias omitted {path} {}",
            child.get_name()
        );
        if let Some(alias_child) = alias_child {
            same_surface(alias_child, child, &format!("{path} {}", child.get_name()));
        }
    }
}

#[test]
fn all_v9_and_renamed_group_aliases_resolve_with_the_canonical_surface() -> TestResult {
    let mut tree = commands::command_tree()?;
    tree.build();
    for (alias, canonical) in inventory()?.aliases {
        if !enabled(&canonical) {
            continue;
        }
        let canonical_command = find(&tree, &canonical).ok_or("canonical group missing")?;
        let alias_command = find(&tree, &alias)
            .ok_or_else(|| format!("missing compatibility alias {alias} for {canonical}"))?;
        same_surface(alias_command, canonical_command, &alias);
        if alias_command.get_name() == alias.rsplit(' ').next().ok_or("alias name")? {
            assert!(
                alias_command.is_hide_set(),
                "historical clone {alias} is visible"
            );
        } else {
            assert!(
                !alias_command
                    .get_visible_aliases()
                    .any(|visible| visible == alias.rsplit(' ').next().unwrap_or_default()),
                "compatibility alias {alias} is visible"
            );
        }
    }
    Ok(())
}

fn snake_aliases(command: &Command, prefix: &str) {
    for child in command.get_subcommands() {
        let name = child.get_name();
        if name.contains('-') {
            let alias = name.replace('-', "_");
            assert!(
                child.get_all_aliases().any(|name| name == alias),
                "missing {prefix} {alias}"
            );
            assert!(
                !child.get_visible_aliases().any(|name| name == alias),
                "visible {prefix} {alias}"
            );
            assert!(
                command
                    .find_subcommand(&alias)
                    .is_some_and(|found| std::ptr::eq(found, child))
            );
        }
        snake_aliases(child, &format!("{prefix} {name}"));
    }
}

#[test]
fn every_hyphenated_installed_command_has_an_unadvertised_snake_case_spelling() -> TestResult {
    let tree = commands::command_tree()?;
    snake_aliases(&tree, "rtao");
    Ok(())
}
