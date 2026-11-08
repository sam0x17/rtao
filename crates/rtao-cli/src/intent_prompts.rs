// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/intent_prompts.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_cli_intent_prompts.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Declarative transformations for richer generated transaction prompts.

use crate::prompt::{
    PromptContext, PromptError, PromptFlow, PromptSpec, Terminal, python_strip, supplied,
};
use serde_json::{Map, Value};

struct Rule {
    field: &'static str,
    before: Option<&'static str>,
    flow: PromptFlow,
}
fn source(field: &'static str, netuid: Option<&str>) -> Rule {
    Rule {
        field,
        before: None,
        flow: PromptFlow::StakeSource {
            netuid_field: netuid.map(String::from),
        },
    }
}
fn target(field: &'static str, amount: &'static str) -> Rule {
    Rule {
        field,
        before: Some(amount),
        flow: PromptFlow::StakeTarget,
    }
}
fn destination(field: &'static str) -> Rule {
    Rule {
        field,
        before: None,
        flow: PromptFlow::DestinationAccount,
    }
}
/// A typed address prompt, for a hotkey that is neither a stake position nor
/// a validator (Python's `_typed`).
fn typed(field: &'static str, before: &'static str) -> Rule {
    Rule {
        field,
        before: Some(before),
        flow: PromptFlow::Plain,
    }
}

/// A gamma deposit's hotkey, typed on a terminal: it is an API key, never a
/// stake position or the wallet's own hotkey.
fn deposit_hotkey() -> Rule {
    Rule {
        field: "hotkey_ss58",
        before: None,
        flow: PromptFlow::Plain,
    }
}

/// Fields an operation requires after prompting instead of falling back to
/// the wallet's own key: a gamma deposit's hotkey is an API key, never the
/// wallet's own hotkey.
fn required_after_prompt(op: &str) -> &'static [&'static str] {
    match op {
        "claim_root_with_hotkey"
        | "mint_gamma"
        | "mint_gamma_with_stake"
        | "close_gamma_deposit"
        | "set_gamma_deposit_spender"
        | "set_gamma_deposit_daily_spend_cap" => &["hotkey_ss58"],
        _ => &[],
    }
}

/// Python's `falls_back_to_wallet`: whether an omitted `field` takes the
/// signing wallet's own key; true for `hotkey_ss58` and `coldkey_ss58`
/// unless the operation's policy requires them.
pub fn falls_back_to_wallet(op: &str, field: &str) -> bool {
    matches!(field, "hotkey_ss58" | "coldkey_ss58") && !required_after_prompt(op).contains(&field)
}
fn rules(op: &str) -> Vec<Rule> {
    match op {
        "remove_stake" | "remove_stake_limit" => vec![source("hotkey_ss58", Some("netuid"))],
        "unstake_all" | "unstake_all_alpha" => vec![source("hotkey_ss58", None)],
        "swap_stake" => vec![source("hotkey_ss58", Some("origin_netuid"))],
        "transfer_stake" => vec![
            source("hotkey_ss58", Some("origin_netuid")),
            destination("dest_coldkey_ss58"),
        ],
        "move_stake" | "move_swap_stake" => vec![
            source("origin_hotkey_ss58", Some("origin_netuid")),
            target("dest_hotkey_ss58", "amount_alpha"),
        ],
        "claim_root_with_hotkey" => vec![Rule {
            field: "hotkey_ss58",
            before: None,
            flow: PromptFlow::ClaimRootSource,
        }],
        "add_stake" | "add_stake_limit" => vec![target("hotkey_ss58", "amount_tao")],
        "transfer" | "transfer_all" => vec![destination("dest_ss58")],
        "mint_gamma"
        | "close_gamma_deposit"
        | "set_gamma_deposit_spender"
        | "set_gamma_deposit_daily_spend_cap" => vec![deposit_hotkey()],
        // Each rule is placed first, so the stake source precedes the hotkey.
        "mint_gamma_with_stake" => vec![
            deposit_hotkey(),
            source("origin_hotkey_ss58", Some("netuid")),
        ],
        "move_gamma" => vec![
            typed("origin_hotkey_ss58", "amount_gamma"),
            typed("destination_hotkey_ss58", "amount_gamma"),
        ],
        _ => Vec::new(),
    }
}

/// Transform prompts in Python's rule order and emit any noninteractive notice.
///
/// Richer flows run only without --yes or the extension signer on a human TTY.
/// Supplied fields stay intact; zero/false/empty-string values are supplied too.
/// An explicitly named wallet hotkey is the stake target of `add_stake` and
/// `add_stake_limit`, so their target picker is skipped (D-085; Python still
/// asks). The omitted field then resolves to that hotkey's public address.
pub fn apply_intent_prompt_policy(
    context: &PromptContext,
    terminal: &mut dyn Terminal,
    op: &str,
    mut missing: Vec<PromptSpec>,
    answers: &Map<String, Value>,
) -> Result<Vec<PromptSpec>, PromptError> {
    if !context.json
        && !context.quiet
        && op == "remove_stake"
        && !supplied(answers, "netuid")
        && answers
            .get("amount_alpha")
            .and_then(Value::as_str)
            .is_some_and(|amount| python_strip(amount).eq_ignore_ascii_case("all"))
    {
        terminal.message("note: `--amount all` unstakes everything on a single subnet (--netuid); to unstake every position across all subnets, use `rtao stake unstake-all`")?;
    }
    if context.assume_yes || context.extension_signer || !context.interactive(terminal) {
        return Ok(missing);
    }
    for rule in rules(op) {
        if supplied(answers, rule.field) {
            continue;
        }
        if wallet_hotkey_target(context, &rule) {
            missing.retain(|spec| spec.field != rule.field);
            continue;
        }
        missing.retain(|spec| spec.field != rule.field);
        let index = rule
            .before
            .and_then(|before| missing.iter().position(|spec| spec.field == before))
            .unwrap_or(if rule.before.is_some() {
                missing.len()
            } else {
                0
            });
        let mut spec = PromptSpec::new(
            rule.field,
            format!(
                "--{}",
                rule.field.trim_end_matches("_ss58").replace('_', "-")
            ),
        );
        spec.flow = rule.flow;
        missing.insert(index, spec);
    }
    if matches!(
        op,
        "add_stake" | "add_stake_limit" | "stake_burn" | "transfer"
    ) {
        for spec in &mut missing {
            if spec.field == "amount_tao" {
                spec.free_balance = true
            }
        }
    }
    Ok(missing)
}

/// Only the canonical wallet-defaulted `hotkey_ss58` target follows a named
/// wallet hotkey; sources, move destinations and claim-root keep their flows.
fn wallet_hotkey_target(context: &PromptContext, rule: &Rule) -> bool {
    context.wallet_hotkey_given
        && rule.field == "hotkey_ss58"
        && rule.flow == PromptFlow::StakeTarget
}

/// Enforce fields that cannot silently fall back after policy prompting.
pub fn validate_intent_prompt_policy(
    op: &str,
    answers: &Map<String, Value>,
) -> Result<(), PromptError> {
    let missing = required_after_prompt(op)
        .iter()
        .filter(|field| !supplied(answers, field))
        .map(|field| format!("--{}", field.trim_end_matches("_ss58").replace('_', "-")))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(PromptError::MissingRequired(missing));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use secrecy::SecretString;
    #[derive(Default)]
    struct TerminalFixture {
        interactive: bool,
        messages: Vec<String>,
    }
    impl Terminal for TerminalFixture {
        fn stdin_is_tty(&self) -> bool {
            self.interactive
        }
        fn stderr_is_tty(&self) -> bool {
            self.interactive
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            Err(crate::prompt::PromptError::Unavailable)
        }
        fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
            Err(crate::prompt::PromptError::Unavailable)
        }
        fn message(&mut self, message: &str) -> Result<(), PromptError> {
            self.messages.push(message.into());
            Ok(())
        }
    }
    fn specs(fields: &[&str]) -> Vec<PromptSpec> {
        fields
            .iter()
            .map(|field| PromptSpec::new(*field, format!("--{}", field.replace('_', "-"))))
            .collect()
    }

    // pyt: unit/test_cli_intent_prompts.py::test_add_stake_policy_places_target_before_decorated_amount
    #[test]
    fn add_stake_policy_places_target_before_decorated_amount() {
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        let result = apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "add_stake",
            specs(&["netuid", "hotkey_ss58", "amount_tao"]),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|spec| spec.field.as_str())
                .collect::<Vec<_>>(),
            ["netuid", "hotkey_ss58", "amount_tao"]
        );
        assert_eq!(result[1].flow, PromptFlow::StakeTarget);
        assert!(result[1].custom());
        // Python wraps the ordinary amount ask with a custom free-balance
        // decorator. Native prompting represents that decorator separately
        // from the picker flow, and the command consumer uses both fields.
        assert_eq!(result[2].flow, PromptFlow::Plain);
        assert!(result[2].free_balance);
    }

    // pyt: unit/test_cli_intent_prompts.py::test_remove_stake_policy_moves_source_picker_first
    #[test]
    fn remove_stake_policy_moves_source_picker_first() {
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        let result = apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "remove_stake",
            specs(&["netuid", "hotkey_ss58", "amount_alpha"]),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|spec| spec.field.as_str())
                .collect::<Vec<_>>(),
            ["hotkey_ss58", "netuid", "amount_alpha"]
        );
        assert_eq!(
            result[0].flow,
            PromptFlow::StakeSource {
                netuid_field: Some("netuid".into())
            }
        );
        assert!(result[0].custom());
    }

    #[test]
    fn move_target_precedes_amount_and_source_preserves_the_subnet_field() {
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        let result = apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "move_stake",
            specs(&["amount_alpha", "origin_hotkey_ss58", "dest_hotkey_ss58"]),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|spec| spec.field.as_str())
                .collect::<Vec<_>>(),
            ["origin_hotkey_ss58", "dest_hotkey_ss58", "amount_alpha"]
        );
        assert_eq!(
            result[0].flow,
            PromptFlow::StakeSource {
                netuid_field: Some("origin_netuid".into())
            }
        );
        assert_eq!(result[1].flow, PromptFlow::StakeTarget);
    }

    // D-085: Python asks for the target even after --wallet-hotkey.
    #[test]
    fn add_stake_policy_uses_named_wallet_hotkey_without_target_picker() {
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        let named = PromptContext {
            wallet_hotkey_given: true,
            ..Default::default()
        };
        for op in ["add_stake", "add_stake_limit"] {
            for missing in [
                specs(&["netuid", "amount_tao"]),
                specs(&["netuid", "hotkey_ss58", "amount_tao"]),
            ] {
                let result =
                    apply_intent_prompt_policy(&named, &mut terminal, op, missing, &Map::new())
                        .unwrap();
                assert_eq!(
                    result
                        .iter()
                        .map(|spec| spec.field.as_str())
                        .collect::<Vec<_>>(),
                    ["netuid", "amount_tao"],
                    "{op}"
                );
                assert!(result[1].free_balance);
            }
            let unnamed = apply_intent_prompt_policy(
                &PromptContext::default(),
                &mut terminal,
                op,
                specs(&["netuid", "amount_tao"]),
                &Map::new(),
            )
            .unwrap();
            assert_eq!(unnamed[1].field, "hotkey_ss58");
            assert_eq!(unnamed[1].flow, PromptFlow::StakeTarget);
        }
        assert!(terminal.messages.is_empty());
    }

    #[test]
    fn named_wallet_hotkey_keeps_source_destination_and_claim_root_pickers() {
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        let named = PromptContext {
            wallet_hotkey_given: true,
            ..Default::default()
        };
        let moved = apply_intent_prompt_policy(
            &named,
            &mut terminal,
            "move_stake",
            specs(&["amount_alpha"]),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(
            moved
                .iter()
                .map(|spec| (spec.field.as_str(), spec.flow.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    "origin_hotkey_ss58",
                    PromptFlow::StakeSource {
                        netuid_field: Some("origin_netuid".into())
                    }
                ),
                ("dest_hotkey_ss58", PromptFlow::StakeTarget),
                ("amount_alpha", PromptFlow::Plain),
            ]
        );
        for (op, subnet) in [
            ("remove_stake", Some("netuid")),
            ("unstake_all", None),
            ("swap_stake", Some("origin_netuid")),
        ] {
            let result =
                apply_intent_prompt_policy(&named, &mut terminal, op, Vec::new(), &Map::new())
                    .unwrap();
            assert_eq!(result[0].field, "hotkey_ss58");
            assert_eq!(
                result[0].flow,
                PromptFlow::StakeSource {
                    netuid_field: subnet.map(String::from)
                }
            );
        }
        let claim = apply_intent_prompt_policy(
            &named,
            &mut terminal,
            "claim_root_with_hotkey",
            Vec::new(),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(claim[0].flow, PromptFlow::ClaimRootSource);
    }

    #[test]
    fn gamma_deposit_hotkeys_never_fall_back_to_the_wallet() {
        for op in [
            "mint_gamma",
            "mint_gamma_with_stake",
            "close_gamma_deposit",
            "set_gamma_deposit_spender",
            "set_gamma_deposit_daily_spend_cap",
            "claim_root_with_hotkey",
        ] {
            assert!(!falls_back_to_wallet(op, "hotkey_ss58"), "{op}");
            assert!(falls_back_to_wallet(op, "coldkey_ss58"), "{op}");
            assert_eq!(
                validate_intent_prompt_policy(op, &Map::new())
                    .unwrap_err()
                    .to_string(),
                PromptError::MissingRequired(vec!["--hotkey".into()]).to_string()
            );
        }
        assert!(falls_back_to_wallet("add_stake", "hotkey_ss58"));
        assert!(!falls_back_to_wallet("move_gamma", "origin_hotkey_ss58"));
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        let source = apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "mint_gamma_with_stake",
            Vec::new(),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(source[0].field, "origin_hotkey_ss58");
        assert_eq!(
            source[0].flow,
            PromptFlow::StakeSource {
                netuid_field: Some("netuid".into())
            }
        );
        let moved = apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "move_gamma",
            specs(&["netuid", "amount_gamma"]),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(
            moved
                .iter()
                .map(|spec| (spec.field.as_str(), spec.flow.clone()))
                .collect::<Vec<_>>(),
            [
                ("netuid", PromptFlow::Plain),
                ("origin_hotkey_ss58", PromptFlow::Plain),
                ("destination_hotkey_ss58", PromptFlow::Plain),
                ("amount_gamma", PromptFlow::Plain),
            ]
        );
    }

    #[test]
    fn registry_matches_all_python_rules_and_amount_decorators() {
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        for (op, source_field, subnet) in [
            ("remove_stake", "hotkey_ss58", Some("netuid")),
            ("remove_stake_limit", "hotkey_ss58", Some("netuid")),
            ("unstake_all", "hotkey_ss58", None),
            ("unstake_all_alpha", "hotkey_ss58", None),
            ("swap_stake", "hotkey_ss58", Some("origin_netuid")),
            (
                "move_swap_stake",
                "origin_hotkey_ss58",
                Some("origin_netuid"),
            ),
        ] {
            let result = apply_intent_prompt_policy(
                &PromptContext::default(),
                &mut terminal,
                op,
                Vec::new(),
                &Map::new(),
            )
            .unwrap();
            assert_eq!(result[0].field, source_field);
            assert_eq!(
                result[0].flow,
                PromptFlow::StakeSource {
                    netuid_field: subnet.map(String::from)
                }
            );
        }
        let transfer = apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "transfer_stake",
            Vec::new(),
            &Map::new(),
        )
        .unwrap();
        assert_eq!(
            transfer
                .iter()
                .map(|spec| spec.field.as_str())
                .collect::<Vec<_>>(),
            ["dest_coldkey_ss58", "hotkey_ss58"]
        );
        for op in ["add_stake", "add_stake_limit", "stake_burn", "transfer"] {
            let result = apply_intent_prompt_policy(
                &PromptContext::default(),
                &mut terminal,
                op,
                specs(&["amount_tao"]),
                &Map::new(),
            )
            .unwrap();
            assert!(
                result
                    .iter()
                    .find(|spec| spec.field == "amount_tao")
                    .unwrap()
                    .free_balance
            );
        }
        for op in ["transfer", "transfer_all"] {
            let result = apply_intent_prompt_policy(
                &PromptContext::default(),
                &mut terminal,
                op,
                Vec::new(),
                &Map::new(),
            )
            .unwrap();
            assert_eq!(result[0].flow, PromptFlow::DestinationAccount);
        }
    }

    #[test]
    fn supplied_fields_and_all_nonprompting_modes_preserve_input_specs() {
        let original = specs(&["amount_alpha", "hotkey_ss58"]);
        let supplied = Map::from_iter([("hotkey_ss58".into(), Value::String(String::new()))]);
        let mut terminal = TerminalFixture {
            interactive: true,
            ..Default::default()
        };
        assert_eq!(
            apply_intent_prompt_policy(
                &PromptContext::default(),
                &mut terminal,
                "remove_stake",
                original.clone(),
                &supplied
            )
            .unwrap(),
            original
        );
        for context in [
            PromptContext {
                assume_yes: true,
                ..Default::default()
            },
            PromptContext {
                extension_signer: true,
                ..Default::default()
            },
            PromptContext {
                json: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                apply_intent_prompt_policy(
                    &context,
                    &mut terminal,
                    "remove_stake",
                    original.clone(),
                    &Map::new()
                )
                .unwrap(),
                original
            );
        }
        terminal.interactive = false;
        assert_eq!(
            apply_intent_prompt_policy(
                &PromptContext::default(),
                &mut terminal,
                "remove_stake",
                original.clone(),
                &Map::new()
            )
            .unwrap(),
            original
        );
    }

    #[test]
    fn all_notice_is_emitted_without_prompts_and_root_hotkey_remains_required() {
        let mut terminal = TerminalFixture::default();
        let answers = Map::from_iter([("amount_alpha".into(), Value::String(" ALL ".into()))]);
        apply_intent_prompt_policy(
            &PromptContext::default(),
            &mut terminal,
            "remove_stake",
            Vec::new(),
            &answers,
        )
        .unwrap();
        assert_eq!(terminal.messages.len(), 1);
        assert!(terminal.messages[0].contains("rtao stake unstake-all"));
        assert_eq!(
            validate_intent_prompt_policy("claim_root_with_hotkey", &Map::new())
                .unwrap_err()
                .exit_code(),
            2
        );
        assert!(
            validate_intent_prompt_policy(
                "claim_root_with_hotkey",
                &Map::from_iter([("hotkey_ss58".into(), Value::String("provided".into()))])
            )
            .is_ok()
        );
        assert!(validate_intent_prompt_policy("other", &Map::new()).is_ok());
    }
}
