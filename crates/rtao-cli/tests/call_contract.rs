// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/call.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Raw-call shield availability through parsed requests and fake transport.

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use rtao::{
    client::{Client, ClientConfig, Network, View},
    primitives::{AccountId, CryptoType},
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
};
use rtao_cli::{
    application::command_tree,
    books::Books,
    call::{self, RawCallRequest},
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Map, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct NoTerminal;
impl Terminal for NoTerminal {
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

struct Keys {
    signer: KeypairSigner,
    unlocks: AtomicUsize,
}
impl KeyView for Keys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.signer.account().0)
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
        })
    }
}

fn request(
    target: &str,
    args: &str,
    forced: bool,
) -> Result<(AppContext, RawCallRequest, MemorySink), Box<dyn std::error::Error>> {
    let mut argv = vec!["rtao", "call", target, "--args", args, "--yes"];
    if forced {
        argv.push("--mev-shield");
    }
    let parsed = command_tree([call::command()]).try_get_matches_from(argv)?;
    let leaf = parsed.subcommand().ok_or("call command")?.1;
    let config = Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/raw-call-contract-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &["signer_backend"], |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(OutputOptions::default(), Arc::new(sink.clone()));
    let request = RawCallRequest::from_matches(&ctx, leaf)?;
    Ok((ctx, request, sink))
}

// py: sdk/python/bittensor/cli/call.py::_honor_shield
#[tokio::test]
async fn absent_raw_shield_key_allows_only_the_unforced_default_to_submit() -> TestResult {
    for (target, forced, required) in [
        ("SubtensorModule.add_stake", false, false),
        ("SubtensorModule.add_stake", true, false),
        ("SubtensorModule.burned_register", false, true),
    ] {
        // Public deterministic fixture seed, never a wallet or private file.
        let keys = Keys {
            signer: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?,
            unlocks: AtomicUsize::new(0),
        };
        let mut params = json!({"netuid":1,"hotkey":keys.signer.account()});
        if !required {
            params["amount_staked"] = json!(1);
        }
        let (ctx, request, sink) = request(target, &params.to_string(), forced)?;
        assert_eq!(call::shield_policy_for_target(target), (true, required));
        let mut state = FakeState::try_default()?;
        let next_key = state
            .runtime
            .storage_entry("MevShield", "NextKey")
            .ok_or("NextKey metadata")?;
        let next_key = state.runtime.storage_key(next_key, &[])?;
        // This storage item is Optional<BoundedVec>; missing storage, rather
        // than an encoded empty vector, represents an inactive shield key.
        state.seed_encoded(next_key, None::<Vec<u8>>);
        state.seed_python(
            "System",
            "Account",
            &[json!(keys.signer.account())],
            json!({"data":{"free":10_000_000_000_u64}}),
        )?;
        let fake = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
        let checked: TestResult = async {
            let runtime = View::runtime(&client).await?;
            let result = call::execute_with(&ctx, &client, &keys, &request, "rtao call").await;
            let submissions = fake
                .state()
                .lock()
                .map_err(|_| "fixture state")?
                .submissions
                .clone();
            if forced || required {
                let error = result.err().ok_or("inactive shield must refuse")?;
                assert_eq!(error.code, 2);
                assert_eq!(
                    error.message,
                    "MEV shield is not active on this network (MevShield.NextKey is unset)"
                );
                let expected_help = if required {
                    format!("{target} cannot submit unshielded")
                } else {
                    "pass --no-mev-shield to submit unshielded".into()
                };
                assert_eq!(error.help.as_deref(), Some(expected_help.as_str()));
                assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
                assert!(submissions.is_empty());
                assert!(
                    fake.requests()
                        .iter()
                        .all(|request| !request.method.starts_with("author_"))
                );
                assert!(!sink.text(Stream::Stderr)?.contains("submitting unshielded"));
            } else {
                assert!(result?.ok_or("raw receipt")?.success);
                assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
                assert_eq!(submissions.len(), 1);
                let decoded = rtao::core::DynValue::new(
                    runtime.core().decode_extrinsic(&submissions[0], true)?,
                )
                .to_json();
                assert_eq!(decoded["call"]["call_module"], "SubtensorModule");
                assert_eq!(decoded["call"]["call_function"], "add_stake");
                assert!(submissions[0].ends_with(call::compose(&runtime, &request)?.as_bytes()));
                assert!(
                    sink.text(Stream::Stderr)?.contains(
                        "MEV shield is not active on this network — submitting unshielded"
                    )
                );
            }
            Ok(())
        }
        .await;
        client.close().await;
        checked?;
    }
    Ok(())
}
