// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/association.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public ownership-proof consent followed by the ordinary native review.

use super::{failure, key_info};
use crate::{
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::ReviewSection,
    prompt::{PromptHistory, WalletConfirmation},
};
use clap::{Arg, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client},
    evm::{EvmAccount, association_proof},
    primitives::{AccountId, NetUid},
    signer::BoxFuture,
};
use rtao_intents::AssociateEvmKey;
use serde_json::{Value, json};

pub(super) fn command() -> Command {
    with_globals(
        Command::new("associate")
            .about("Associate an EVM key with the wallet hotkey on a subnet, proof included.")
            .long_about("Associate an EVM key with a hotkey on a subnet. The EVM key signs an EIP-191 ownership proof over the hotkey public key and the hash of the SCALE u64 block number. The block comes from the finalized Substrate connection, not EVM RPC. Review and confirm proof generation before either private key is unlocked; the complete native transaction is reviewed separately afterwards.")
            .arg(
                Arg::new("netuid")
                    .long("netuid")
                    .required(true)
                    .value_parser(clap::value_parser!(u16))
                    .help("Subnet on which the EVM key association is recorded."),
            )
            .arg(
                Arg::new("evm_key")
                    .long("evm-key")
                    .help("Stored EVM key: NAME or WALLET/NAME; defaults to the wallet's default key."),
            ),
        Tier::Tx,
    )
}

pub(super) fn reserved() -> Vec<&'static str> {
    vec!["netuid", "evm_key"]
}

/// Proof generation needs the EVM private key, so it has its own public review
/// and consent before the normal exact native transaction review. A dry run
/// contains the unsigned proof message and reports that the proof is required;
/// it cannot contain a produced signature or an exact signed-call fee estimate.
pub(super) async fn run(
    ctx: &mut AppContext,
    client: &Client,
    leaf: &ArgMatches,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let netuid = NetUid(
        *leaf
            .get_one::<u16>("netuid")
            .ok_or_else(|| CliError::input("missing required option `--netuid`"))?,
    );
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: "Wallet containing the signing hotkey.".into(),
            require_coldkey: false,
            ..Default::default()
        },
        history,
    )?;
    let key = leaf.get_one::<String>("evm_key").map(String::as_str);
    let info = key_info(ctx, key).await?;
    let public = ctx.clone();
    let hotkey = rtao::core::offload(move || proof_hotkey(&public))
        .await
        .map_err(failure)??;
    let block = client
        .at(BlockRef::Finalized)
        .await
        .map_err(CliError::from)?;
    let request = ProofRequest {
        netuid,
        evm_address: info.address,
        hotkey,
        block_number: block.block().number,
    };
    let mut backend = NativeBackend {
        client,
        key,
        options,
        external,
        history,
    };
    run_prepared(ctx, &request, &mut backend).await
}

/// The proof authenticates the account dispatching associate_evm_key. Proxy
/// delegates and saved multisig members sign the envelope for that account.
fn proof_hotkey(ctx: &AppContext) -> Result<AccountId, CliError> {
    let account = if let Some(account) = ctx.dispatch_proxy(None)? {
        Some(account)
    } else if ctx.books.multisig(&ctx.globals.wallet_name).is_some() {
        ctx.resolve_address("coldkey_ss58", None)?
    } else {
        if !ctx.globals.signatories.is_empty() {
            return Err(CliError::input(
                "--signatory requires a saved multisig wallet",
            ));
        }
        if ctx.globals.uses_external_signer() {
            ctx.external_signer_address()?
        } else {
            ctx.resolve_address("hotkey_ss58", None)?
        }
    };
    account.ok_or_else(|| {
        CliError::input("EVM association needs the public hotkey account")
            .with_help("select a wallet hotkey or pass --signer-address for an external signer")
    })
}

struct ProofRequest {
    netuid: NetUid,
    evm_address: String,
    hotkey: AccountId,
    block_number: u64,
}
impl ProofRequest {
    fn summary(&self) -> String {
        format!(
            "associate EVM key {} with the hotkey on netuid {}",
            self.evm_address, self.netuid
        )
    }

    fn rows(&self) -> Vec<(String, Value)> {
        vec![
            ("EVM key".into(), json!(self.evm_address)),
            ("hotkey".into(), json!(self.hotkey)),
            ("netuid".into(), json!(self.netuid)),
            ("finalized block".into(), json!(self.block_number)),
            ("proof scheme".into(), json!("EIP-191 personal-sign")),
            (
                "proof message".into(),
                json!(format!(
                    "0x{}",
                    hex::encode(rtao::evm::transactions::association_message(
                        self.hotkey,
                        self.block_number,
                    ))
                )),
            ),
            (
                "next step".into(),
                json!(
                    "Generate the EVM proof, then review the complete native transaction and fee."
                ),
            ),
        ]
    }

    fn preview(&self) -> Value {
        json!({
            "op": "associate_evm_key",
            "stage": "ownership_proof",
            "summary": self.summary(),
            "hotkey": self.hotkey,
            "args": {
                "netuid": self.netuid,
                "evm_key": self.evm_address,
                "block_number": self.block_number,
                "signature": null,
            },
            "proof": {
                "scheme": "EIP-191 personal-sign",
                "message": format!("0x{}", hex::encode(rtao::evm::transactions::association_message(self.hotkey, self.block_number))),
                "status": "required",
            },
            "fee_tao": null,
            "submitted": false,
            "warnings": ["Ownership proof generation requires the EVM private key. The complete native transaction and fee are reviewed after proof generation."],
        })
    }
}

trait AssociationBackend: Send {
    fn unlock<'a>(&'a mut self, ctx: &'a AppContext)
    -> BoxFuture<'a, Result<EvmAccount, CliError>>;

    fn submit<'a>(
        &'a mut self,
        ctx: &'a mut AppContext,
        intent: &'a AssociateEvmKey,
    ) -> BoxFuture<'a, Result<(), CliError>>;
}

struct NativeBackend<'a> {
    client: &'a Client,
    key: Option<&'a str>,
    options: SubmissionOptions<'a>,
    external: Option<&'a dyn ExternalSignerFactory>,
    history: &'a mut PromptHistory,
}
impl AssociationBackend for NativeBackend<'_> {
    fn unlock<'a>(
        &'a mut self,
        ctx: &'a AppContext,
    ) -> BoxFuture<'a, Result<EvmAccount, CliError>> {
        Box::pin(async move { super::keys::unlock_key(ctx, self.key).await })
    }

    fn submit<'a>(
        &'a mut self,
        ctx: &'a mut AppContext,
        intent: &'a AssociateEvmKey,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            ctx.submit_with_options(
                self.client,
                intent,
                self.options,
                self.external,
                self.history,
            )
            .await?;
            Ok(())
        })
    }
}

async fn run_prepared(
    ctx: &mut AppContext,
    request: &ProofRequest,
    backend: &mut dyn AssociationBackend,
) -> Result<(), CliError> {
    if ctx.globals.dry_run {
        return ctx
            .output
            .detail(
                Some("EVM association proof required"),
                &request.rows(),
                Some(&request.preview()),
            )
            .map_err(failure);
    }
    ctx.output
        .transaction_card(&[ReviewSection {
            title: "EVM ownership proof".into(),
            rows: request.rows(),
        }])
        .map_err(failure)?;
    ctx.confirm(&format!(
        "Generate the EVM ownership proof to {}?",
        request.summary()
    ))?;
    let (signature, block_number) = {
        let account = backend.unlock(ctx).await?;
        let unlocked =
            rtao::evm::normalize_h160(&account.address().to_string()).map_err(failure)?;
        let reviewed = rtao::evm::normalize_h160(&request.evm_address).map_err(failure)?;
        if unlocked != reviewed {
            return Err(CliError::failure(
                "unlocked EVM key does not match the reviewed public address",
            ));
        }
        association_proof(&account, &request.hotkey.to_string(), request.block_number)
            .map_err(failure)?
    };
    let intent = AssociateEvmKey {
        netuid: request.netuid,
        evm_key: request.evm_address.clone(),
        block_number,
        signature,
    };
    // Keep normal confirmation, semantic wrapping, external-source release and
    // durable receipt callbacks; proof consent does not approve the native call.
    backend.submit(ctx, &intent).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::command_tree,
        books::Books,
        globals::Globals,
        output::{MemorySink, Output, OutputOptions, Stream},
        prompt::{PromptError, Terminal},
    };
    use std::{
        collections::VecDeque,
        path::Path,
        sync::{Arc, Mutex},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type Questions = Arc<Mutex<Vec<String>>>;
    type Fixture = (AppContext, MemorySink, Questions);

    struct FakeTerminal {
        answers: VecDeque<String>,
        questions: Questions,
    }
    impl Terminal for FakeTerminal {
        fn stdin_is_tty(&self) -> bool {
            true
        }
        fn stderr_is_tty(&self) -> bool {
            true
        }
        fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
            self.questions
                .lock()
                .map_err(|_| PromptError::Io)?
                .push(label.into());
            self.answers.pop_front().ok_or(PromptError::Aborted)
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, _: &str) -> Result<(), PromptError> {
            Ok(())
        }
    }

    fn context(flags: &[&str], answers: &[&str]) -> Result<Fixture, Box<dyn std::error::Error>> {
        let matches = command_tree([command()]).try_get_matches_from(
            ["rtao", "associate", "--netuid", "12"]
                .into_iter()
                .chain(flags.iter().copied()),
        )?;
        let mut globals = Globals::resolve(
            &matches,
            &Default::default(),
            Path::new("/isolated/home"),
            |_| None,
        )?;
        globals.apply(matches.subcommand().ok_or("command")?.1, &reserved())?;
        let questions = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(FakeTerminal {
                answers: answers.iter().map(|answer| (*answer).into()).collect(),
                questions: questions.clone(),
            }))),
        );
        let sink = MemorySink::default();
        ctx.output = Output::new(
            OutputOptions {
                json_mode: ctx.globals.json,
                ..Default::default()
            },
            Arc::new(sink.clone()),
        );
        Ok((ctx, sink, questions))
    }

    struct FakeBackend {
        unlocks: usize,
        submitted: Option<AssociateEvmKey>,
        mismatch: bool,
        native_refused: bool,
    }
    impl FakeBackend {
        fn new() -> Self {
            Self {
                unlocks: 0,
                submitted: None,
                mismatch: false,
                native_refused: false,
            }
        }
    }
    impl AssociationBackend for FakeBackend {
        fn unlock<'a>(
            &'a mut self,
            _: &'a AppContext,
        ) -> BoxFuture<'a, Result<EvmAccount, CliError>> {
            Box::pin(async move {
                self.unlocks += 1;
                let value = if self.mismatch { "02" } else { "01" };
                value.repeat(32).parse().map_err(failure)
            })
        }
        fn submit<'a>(
            &'a mut self,
            ctx: &'a mut AppContext,
            intent: &'a AssociateEvmKey,
        ) -> BoxFuture<'a, Result<(), CliError>> {
            Box::pin(async move {
                // The native layer still gets its own independent consent.
                ctx.confirm("Submit the complete native association?")?;
                if self.native_refused {
                    return Err(CliError::failure("native preflight refused"));
                }
                self.submitted = Some(intent.clone());
                Ok(())
            })
        }
    }

    fn request() -> ProofRequest {
        ProofRequest {
            netuid: NetUid(12),
            evm_address: "0x1a642f0e3c3af545e7acbd38b07251b3990914f1".into(),
            hotkey: AccountId([7; 32]),
            block_number: (1 << 32) + 73,
        }
    }

    #[tokio::test]
    async fn refused_and_dry_run_never_unlock_or_submit() -> TestResult {
        for (flags, answers, fails) in [
            (vec![], vec!["no"], true),
            (vec!["--json"], vec![], true),
            (vec!["--dry-run", "--json"], vec![], false),
            (vec!["--dry-run", "--yes"], vec![], false),
        ] {
            let (mut ctx, sink, questions) = context(&flags, &answers)?;
            let mut backend = FakeBackend::new();
            let result = run_prepared(&mut ctx, &request(), &mut backend).await;
            assert_eq!(result.is_err(), fails);
            assert_eq!(backend.unlocks, 0);
            assert!(backend.submitted.is_none());
            if flags.contains(&"--dry-run") {
                assert!(questions.lock().map_err(|_| "questions")?.is_empty());
                if flags.contains(&"--json") {
                    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
                    assert_eq!(payload["proof"]["status"], "required");
                    assert!(payload["args"]["signature"].is_null());
                    assert_eq!(payload["submitted"], false);
                    assert_eq!(payload["args"]["block_number"], request().block_number);
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn consent_signs_exact_reviewed_hotkey_block_and_routes_native_intent() -> TestResult {
        let (mut ctx, _, questions) = context(&[], &["yes", "yes"])?;
        let mut backend = FakeBackend::new();
        let request = request();
        run_prepared(&mut ctx, &request, &mut backend).await?;
        let intent = backend.submitted.ok_or("native intent")?;
        assert_eq!(backend.unlocks, 1);
        assert_eq!(intent.netuid, request.netuid);
        assert_eq!(intent.evm_key, request.evm_address);
        assert_eq!(intent.block_number, request.block_number);
        let signature = hex::decode(intent.signature.trim_start_matches("0x"))?;
        assert_eq!(signature.len(), 65);
        let recovered = rtao::evm::transactions::recover_association_owner(
            request.hotkey,
            request.block_number,
            &signature,
        )?;
        assert_eq!(
            rtao::evm::normalize_h160(&recovered.to_string())?,
            request.evm_address
        );
        let changed = rtao::evm::transactions::recover_association_owner(
            AccountId([8; 32]),
            request.block_number,
            &signature,
        )?;
        assert_ne!(changed, recovered);
        assert_eq!(questions.lock().map_err(|_| "questions")?.len(), 2);
        assert!(!ctx.globals.assume_yes);
        Ok(())
    }

    #[tokio::test]
    async fn native_refusal_and_swapped_evm_key_cannot_submit() -> TestResult {
        let (mut ctx, _, _) = context(&[], &["yes", "no"])?;
        let mut backend = FakeBackend::new();
        assert!(
            run_prepared(&mut ctx, &request(), &mut backend)
                .await
                .is_err()
        );
        assert_eq!(backend.unlocks, 1);
        assert!(backend.submitted.is_none());

        let (mut ctx, _, questions) = context(&[], &["yes"])?;
        let mut backend = FakeBackend::new();
        backend.mismatch = true;
        let error = run_prepared(&mut ctx, &request(), &mut backend)
            .await
            .err()
            .ok_or("mismatched key must refuse")?;
        assert!(error.message.contains("does not match"));
        assert!(backend.submitted.is_none());
        assert_eq!(questions.lock().map_err(|_| "questions")?.len(), 1);

        let (mut ctx, _, _) = context(&["--yes"], &[])?;
        let mut backend = FakeBackend::new();
        backend.native_refused = true;
        assert!(
            run_prepared(&mut ctx, &request(), &mut backend)
                .await
                .is_err()
        );
        assert_eq!(backend.unlocks, 1);
        assert!(backend.submitted.is_none());
        Ok(())
    }

    #[test]
    fn proof_authenticates_proxy_before_multisig_and_external_envelopes() -> TestResult {
        let (mut ctx, _, _) = context(&[], &[])?;
        ctx.globals.wallet_name = "treasury".into();
        ctx.books = Books::from_values(
            json!([]),
            json!([]),
            json!([{
                "name": "treasury",
                "threshold": 2,
                "signatories": [
                    "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
                    "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty",
                ],
            }]),
        );
        // Recorded upstream multisig vector, also used by the resolver tests.
        let multisig: AccountId = "5F3QVbS78a4aTYLiRAD8N3czjqVoNyV42L19CXyhqUMCh4Ch".parse()?;
        assert_eq!(proof_hotkey(&ctx)?, multisig);

        let configured = AccountId([9; 32]);
        ctx.config
            .insert("proxy_for".into(), json!(configured.to_string()));
        assert_eq!(proof_hotkey(&ctx)?, configured);

        let explicit = AccountId([10; 32]);
        ctx.globals.proxy_for = Some(explicit.to_string());
        assert_eq!(proof_hotkey(&ctx)?, explicit);
        ctx.globals.proxy_for = Some("self".into());
        assert_eq!(proof_hotkey(&ctx)?, multisig);

        ctx.books = Books::default();
        ctx.globals.signer_backend = Some("ledger".into());
        let external = AccountId([11; 32]);
        ctx.globals.signer_address = Some(external.to_string());
        assert_eq!(proof_hotkey(&ctx)?, external);
        ctx.globals.proxy_for = None;
        assert_eq!(proof_hotkey(&ctx)?, configured);
        Ok(())
    }

    #[test]
    fn native_netuid_is_checked_before_runtime_work() {
        assert!(
            command()
                .try_get_matches_from(["associate", "--netuid", "65535"])
                .is_ok()
        );
        assert!(
            command()
                .try_get_matches_from(["associate", "--netuid", "65536"])
                .is_err()
        );
        assert!(
            command()
                .try_get_matches_from(["associate", "--netuid", "-1"])
                .is_err()
        );
        assert!(command().try_get_matches_from(["associate"]).is_err());
    }
}
