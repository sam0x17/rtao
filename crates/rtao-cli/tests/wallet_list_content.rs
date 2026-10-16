//! The actual wallet list retains members and per-entry public metadata.

use rtao::primitives::AccountId;
use rtao_cli::{
    application,
    books::{Books, FsAccountLookup, NameResolver},
    commands::wallet,
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
struct NoInput;
impl Terminal for NoInput {
    fn stdin_is_tty(&self) -> bool {
        false
    }
    fn stderr_is_tty(&self) -> bool {
        false
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

#[tokio::test]
async fn actual_wallet_list_keeps_multisig_members_and_later_optional_fields_in_human_and_json()
-> TestResult {
    let task_root = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT").ok_or("hosted tests require RTAO_TEMP_ROOT")?,
    );
    let absent = task_root
        .join("tmp")
        .join(format!("wallet-list-no-files-{}", std::process::id()));
    if absent.try_exists()? {
        return Err("fixture public wallet path must be absent".into());
    }
    let alice = AccountId([7; 32]).to_string();
    let bob = AccountId([8; 32]).to_string();
    let proxy = AccountId([9; 32]).to_string();
    let hostile_note = "later note\nforged\x1b[2J";
    let books = Books::from_values(
        json!([
            {"name":"member-a","address":alice},
            {"name":"member-b","address":bob,"signer":"ledger","note":hostile_note,"unrendered-extra":"unrecognized-private-sentinel"},
        ]),
        json!([
            {"name":"first-proxy","address":proxy},
            {"name":"second-proxy","address":bob,"proxy_type":"Governance","delay":99,"spawner":alice,"note":"second-proxy-note"},
        ]),
        json!([
            {"name":"safe","threshold":2,"signatories":["member-a","member-b"],"note":"safe member set"},
            {"name":"safe-partial","threshold":"2","signatories":["member-a","member-b","missing"],"note":"partial member set"},
        ]),
    );
    let (expected_account, _) = rtao::core::extrinsic::multisig_account_id(&[[7; 32], [8; 32]], 2)?;
    let expected_address = AccountId(expected_account).to_string();
    let lookup = FsAccountLookup::new(absent.clone())?;
    let resolver = NameResolver::new(&books, &lookup, "default", "default");
    assert!(resolver.saved_multisig_address("safe-partial").is_err());
    for json_mode in [false, true] {
        let matches = application::command_tree([wallet::command()])
            .try_get_matches_from(["rtao", "wallet", "list"])?;
        let group = matches.subcommand().ok_or("wallet group")?.1;
        let leaf = group.subcommand().ok_or("list leaf")?.1;
        let mut globals = Globals::resolve(&matches, &Default::default(), &task_root, |_| None)?;
        globals.apply_with_env(leaf, &[], |_| None)?;
        globals.wallet_path = absent.clone();
        let mut app = AppContext::new(
            globals,
            Default::default(),
            books.clone(),
            Arc::new(Mutex::new(Box::new(NoInput))),
        );
        let sink = MemorySink::default();
        app.output = Output::new(
            OutputOptions {
                json_mode,
                quiet: true,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        wallet::run(&mut app, group, &mut PromptHistory::default()).await?;
        let text = sink.text(Stream::Stdout)?;
        if json_mode {
            let payload: Value = serde_json::from_str(&text)?;
            assert_eq!(payload["path"], json!(absent));
            assert_eq!(payload["coldkeys"], json!([]));
            assert_eq!(payload["addresses"], json!(books.addresses));
            assert_eq!(payload["proxies"], json!(books.proxies));
            let members = payload["multisigs"][0]["signatories"]
                .as_array()
                .ok_or("members")?;
            assert_eq!(
                members,
                json!([{"name":"member-a","ss58":alice},{"name":"member-b","ss58":bob}])
                    .as_array()
                    .ok_or("expected members")?
            );
            assert_eq!(payload["multisigs"][0]["ss58"], expected_address);
            assert_eq!(
                payload["multisigs"][1],
                json!({
                    "name":"safe-partial", "kind":"multisig", "threshold":2,
                    "signatory_count":3, "ss58":expected_address,
                    "signatories":[
                        {"name":"member-a","ss58":alice},
                        {"name":"member-b","ss58":bob},
                        {"name":"missing","ss58":null}
                    ],
                    "note":"partial member set"
                })
            );
        } else {
            let member_end = text.find("addresses").ok_or("addresses section")?;
            let multisig_section = &text[..member_end];
            for expected in [
                "member-a",
                "member-b",
                &alice,
                &bob,
                "signatories",
                "safe member set",
                "safe-partial",
                "missing",
                "partial member set",
                &expected_address,
            ] {
                assert!(
                    multisig_section.contains(expected),
                    "missing member fact {expected}"
                );
            }
            for expected in [
                "ledger",
                r"later note\nforged\x1b[2J",
                "second-proxy-note",
                "99",
                "Governance",
                "Staking",
                &alice,
            ] {
                assert!(
                    text.contains(expected),
                    "missing later entry fact {expected}"
                );
            }
            assert!(!text.contains("unrecognized-private-sentinel"));
            assert!(!text.contains('\x1b'));
            assert!(text.contains(
                "0 coldkeys  ·  0 hotkeys  ·  2 multisigs  ·  2 addresses  ·  2 proxies"
            ));
        }
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert!(!absent.try_exists()?);
    Ok(())
}
