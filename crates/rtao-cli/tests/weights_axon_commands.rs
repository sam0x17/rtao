// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/axon.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Operator command inputs and real semantic submission over in-process RPC.

use rtao::{
    client::{BlockRef, Client, ClientConfig, DynView, Network, View},
    primitives::{AccountId, CryptoType, MechId, NetUid, NetUidStorageIndex},
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
    tx::{BuildCx, Clock, OriginView},
};
use rtao_cli::{
    application,
    books::Books,
    commands::{axon, generated, weights},
    context::{AppContext, SubmissionOptions},
    globals::Globals,
    output::{MemorySink, Output, OutputOptions},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const NETUID: NetUid = NetUid(3);
const MECHID: MechId = MechId(1);

fn tree() -> clap::Command {
    application::command_tree([weights::command(), axon::command()])
}

fn group(parsed: &clap::ArgMatches) -> Result<&clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(parsed.subcommand().ok_or("family")?.1)
}

fn request(parsed: &clap::ArgMatches) -> Result<generated::TxRequest, Box<dyn std::error::Error>> {
    let (family, matches) = parsed.subcommand().ok_or("family")?;
    Ok(match family {
        "weights" => weights::request(matches)?,
        "axon" => axon::request(matches)?,
        _ => return Err("unexpected family".into()),
    })
}

#[test]
fn weights_csv_preserves_order_python_numeric_syntax_defaults_and_leaf_globals() -> TestResult {
    let parsed = tree().try_get_matches_from([
        "rtao",
        "weights",
        "set",
        "--netuid",
        "3",
        "--uids",
        " ２,,0, 1,",
        "--weights",
        " 1_2.5,,.25, 5e-1,",
        "--json",
        "--yes",
        "--period",
        "64",
    ])?;
    let request = request(&parsed)?;
    assert_eq!(request.op, "set_weights");
    assert_eq!(request.args["uids"], json!([2, 0, 1]));
    assert_eq!(request.args["weights"], json!([12.5, 0.25, 0.5]));
    assert_eq!(request.args["mechid"], json!(0));
    assert_eq!(request.args["version_key"], json!(0));
    assert_eq!(request.args["raw_u16"], json!(false));
    assert_eq!(request.period, Some(64));
    let leaf = group(&parsed)?.subcommand().ok_or("leaf")?.1;
    assert!(leaf.get_flag("json"));
    assert!(leaf.get_flag("assume_yes"));
    Ok(())
}

#[test]
fn weights_file_uses_sdk_mapping_canonicalization_for_full_rows() -> TestResult {
    let parsed = tree().try_get_matches_from([
        "rtao",
        "weights",
        "commit",
        "--netuid",
        "3",
        "--weights-file",
        "/fixtures/weights.json",
        "--version-key",
        "18446744073709551615",
        "--raw-u16",
    ])?;
    let rows = (0..2500)
        .rev()
        .map(|uid| (uid.to_string(), json!(uid + 1)))
        .collect::<serde_json::Map<_, _>>();
    let request = weights::request_with_reader(group(&parsed)?, |path| {
        assert_eq!(path, Path::new("/fixtures/weights.json"));
        Ok(Value::Object(rows).to_string())
    })?;
    assert_eq!(request.op, "commit_weights");
    assert_eq!(request.args["version_key"], json!(u64::MAX));
    let intent = request.intent()?;
    let canonical = intent.as_dyn().to_json_dyn();
    assert_eq!(canonical["uids"].as_array().ok_or("uids")?.len(), 2500);
    assert_eq!(canonical["uids"][0], json!(0));
    assert_eq!(canonical["uids"][2499], json!(2499));
    assert_eq!(canonical["weights"][0], json!(1.0));
    assert_eq!(canonical["weights"][2499], json!(2500.0));
    assert_eq!(canonical["commit_reveal_version"], json!(4));
    Ok(())
}

#[test]
fn weights_file_exclusivity_missing_sources_and_invalid_json_refuse_before_read_or_connect()
-> TestResult {
    for extra in [vec!["--uids", "1"], vec!["--weights", "1"]] {
        let mut args = vec![
            "rtao",
            "weights",
            "set",
            "--netuid",
            "3",
            "--weights-file",
            "/unused",
        ];
        args.extend(extra);
        let parsed = tree().try_get_matches_from(args)?;
        let error = weights::request_with_reader(group(&parsed)?, |_| {
            Err(std::io::Error::other("reader must not be invoked"))
        })
        .err()
        .ok_or("must refuse")?;
        assert_eq!(error.code, 2);
        assert_eq!(
            error.message,
            "Use --weights-file or --uids/--weights, not both."
        );
    }
    for extra in [vec![], vec!["--uids", "1"], vec!["--weights", "1"]] {
        let mut args = vec!["rtao", "weights", "set", "--netuid", "3"];
        args.extend(extra);
        let parsed = tree().try_get_matches_from(args)?;
        let error = weights::request(group(&parsed)?)
            .err()
            .ok_or("must refuse")?;
        assert_eq!(error.code, 2);
        assert_eq!(
            error.message,
            "Provide --weights-file or both --uids and --weights."
        );
    }
    let parsed = tree().try_get_matches_from([
        "rtao",
        "weights",
        "set",
        "--netuid",
        "3",
        "--weights-file",
        "/unused",
    ])?;
    for content in ["[]", "null", "true", "1", "\"text\""] {
        let error = weights::request_with_reader(group(&parsed)?, |_| Ok(content.into()))
            .err()
            .ok_or("object required")?;
        assert_eq!(error.code, 2);
        assert_eq!(
            error.message,
            "Weights file must contain a UID-to-weight JSON object."
        );
    }
    for content in ["{", "{\"1\": NaN}"] {
        let error = weights::request_with_reader(group(&parsed)?, |_| Ok(content.into()))
            .err()
            .ok_or("invalid JSON")?;
        assert!(error.message.starts_with("Cannot read weights file:"));
    }
    let error = weights::request_with_reader(group(&parsed)?, |_| {
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    })
    .err()
    .ok_or("read error")?;
    assert!(error.message.starts_with("Cannot read weights file:"));
    Ok(())
}

#[test]
fn weights_native_ranges_duplicates_raw_syntax_and_nonfinite_values_refuse() -> TestResult {
    for (uids, values, raw) in [
        ("1,1", "1,2", false),
        ("65536", "1", false),
        ("-1", "1", false),
        ("1", "NaN", false),
        ("1", "1e9999", false),
        ("1", "1.0", true),
        ("1", "65536", true),
    ] {
        let mut args = vec![
            "rtao",
            "weights",
            "set",
            "--netuid",
            "3",
            "--uids",
            uids,
            "--weights",
            values,
        ];
        if raw {
            args.push("--raw-u16");
        }
        let parsed = tree().try_get_matches_from(args)?;
        assert_eq!(
            weights::request(group(&parsed)?)
                .err()
                .ok_or("must refuse")?
                .code,
            2
        );
    }
    let parsed = tree().try_get_matches_from([
        "rtao",
        "weights",
        "set",
        "--netuid",
        "3",
        "--weights-file",
        "/unused",
    ])?;
    let error = weights::request_with_reader(group(&parsed)?, |_| Ok("{\"01\":1,\"1\":2}".into()))
        .err()
        .ok_or("duplicate normalized UID")?;
    assert!(error.message.contains("Duplicate UIDs"));
    Ok(())
}

#[test]
fn legacy_reveal_has_independent_salt_and_no_mechanism_or_raw_options() -> TestResult {
    let parsed = tree().try_get_matches_from([
        "rtao",
        "weights",
        "reveal",
        "--netuid",
        "3",
        "--uids",
        "2,0",
        "--weights",
        "0.25,1",
        "--salt",
        "7,,65_535,",
        "--version-key",
        "9",
    ])?;
    let request = request(&parsed)?;
    assert_eq!(request.op, "reveal_weights");
    assert_eq!(request.args["salt"], json!([7, 65535]));
    assert_eq!(request.args["version_key"], json!(9));
    for option in ["--raw-u16", "--mechid"] {
        assert!(
            tree()
                .try_get_matches_from([
                    "rtao",
                    "weights",
                    "reveal",
                    "--netuid",
                    "3",
                    "--uids",
                    "1",
                    "--weights",
                    "1",
                    "--salt",
                    "1",
                    option,
                ])
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn axon_tls_switch_defaults_and_native_ranges_match_python_routes() -> TestResult {
    for (tls, expected) in [(false, "serve_axon"), (true, "serve_axon_tls")] {
        let mut args = vec![
            "rtao",
            "axon",
            "set",
            "--netuid",
            "3",
            "--ip",
            "2001:db8::1",
            "--port",
            "443",
            "--certificate",
            "0x01ab",
        ];
        if tls {
            args.push("--tls");
        }
        let parsed = tree().try_get_matches_from(args)?;
        let request = request(&parsed)?;
        assert_eq!(request.op, expected);
        assert_eq!(request.args.contains_key("certificate"), tls);
        let canonical = request.intent()?.as_dyn().to_json_dyn();
        assert_eq!(canonical["protocol"], json!(4));
        assert_eq!(canonical["version"], json!(1));
    }
    let parsed = tree().try_get_matches_from(["rtao", "axon", "reset", "--netuid", "3"])?;
    assert_eq!(request(&parsed)?.op, "reset_axon");
    for invalid in ["65536", "-1"] {
        assert!(
            tree()
                .try_get_matches_from([
                    "rtao",
                    "axon",
                    "set",
                    "--netuid",
                    "3",
                    "--ip",
                    "127.0.0.1",
                    "--port",
                    invalid,
                ])
                .is_err()
        );
    }
    Ok(())
}

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
    key: KeypairSigner,
    unlocks: AtomicUsize,
}
impl Keys {
    fn new() -> Result<Self, SignerError> {
        Ok(Self {
            key: KeypairSigner::from_seed(&[21; 32], CryptoType::Ed25519)?,
            unlocks: AtomicUsize::new(0),
        })
    }
}
impl KeyView for Keys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.key.account().0)
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            assert_eq!(role, SignerRole::Hotkey);
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}

fn context(parsed: &clap::ArgMatches) -> Result<AppContext, Box<dyn std::error::Error>> {
    let leaf = group(parsed)?.subcommand().ok_or("leaf")?.1;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/operator-no-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &[], |_| None)?;
    let mut context = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    context.output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: true,
            network: None,
        },
        Arc::new(MemorySink::default()),
    );
    Ok(context)
}

fn state(keys: &Keys, null: bool, enabled: bool) -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":1_000_000_000_000_u64}}),
    )?;
    state.seed_typed(
        "SubtensorModule",
        "Uids",
        (NETUID, keys.key.account()),
        2_u16,
    )?;
    state.seed_typed(
        "SubtensorModule",
        "CommitRevealWeightsEnabled",
        (NETUID,),
        enabled,
    )?;
    state.seed_typed("SubtensorModule", "WeightsSetRateLimit", (NETUID,), 3_u64)?;
    state.seed_typed(
        "SubtensorModule",
        "LastUpdate",
        (NetUidStorageIndex::new(NETUID, MECHID)?,),
        vec![0_u64, 0, 97],
    )?;
    state.seed_typed(
        "SubtensorModule",
        "LastUpdate",
        (NetUidStorageIndex(NETUID.0),),
        vec![0_u64, 0, 100],
    )?;
    state.seed_typed("SubtensorModule", "MinAllowedWeights", (NETUID,), 2_u16)?;
    state.seed_typed("SubtensorModule", "MaxWeightsLimit", (NETUID,), u16::MAX)?;
    state.seed_python(
        "SubtensorModule",
        "SubnetEpochConsensus",
        &[json!(NETUID)],
        if null {
            json!({"Null":null})
        } else {
            json!({"Yuma":null})
        },
    )?;
    state.seed_typed(
        "SubtensorModule",
        "ValidatorPermit",
        (NETUID,),
        vec![false, false, true],
    )?;
    state.seed_typed("SubtensorModule", "Tempo", (NETUID,), 20_u16)?;
    state.seed_typed("SubtensorModule", "RevealPeriodEpochs", (NETUID,), 1_u64)?;
    state.seed_typed("SubtensorModule", "LastEpochBlock", (NETUID,), 90_u64)?;
    state.seed_typed("SubtensorModule", "PendingEpochAt", (NETUID,), 0_u64)?;
    state.seed_typed("SubtensorModule", "SubnetEpochIndex", (NETUID,), 5_u64)?;
    state.seed_typed("SubtensorModule", "BlocksSinceLastStep", (NETUID,), 15_u64)?;
    Ok(state)
}

async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok((
        Client::from_rpc(RpcClient::new(chain.clone()), config).await?,
        chain,
    ))
}

#[tokio::test]
async fn operator_commands_submit_semantic_bytes_with_hotkey_and_requested_era() -> TestResult {
    for args in [
        vec![
            "rtao",
            "weights",
            "set",
            "--netuid",
            "3",
            "--mechid",
            "1",
            "--uids",
            "0,1,2",
            "--weights",
            "1,2,3",
            "--version-key",
            "9",
        ],
        vec![
            "rtao",
            "weights",
            "reveal",
            "--netuid",
            "3",
            "--uids",
            "2,0",
            "--weights",
            "0.25,1",
            "--salt",
            "7,8",
            "--version-key",
            "9",
        ],
        vec![
            "rtao",
            "axon",
            "set",
            "--netuid",
            "3",
            "--ip",
            "203.0.113.9",
            "--port",
            "8080",
        ],
        vec![
            "rtao",
            "axon",
            "set",
            "--netuid",
            "3",
            "--ip",
            "2001:db8::1",
            "--port",
            "443",
            "--tls",
            "--certificate",
            "0x01 ab",
        ],
        vec!["rtao", "axon", "reset", "--netuid", "3"],
    ] {
        let keys = Keys::new()?;
        let (client, chain) = client(state(&keys, false, false)?).await?;
        let mut args = args;
        args.extend(["--period", "64", "--json", "--yes", "--no-mev-shield"]);
        let parsed = tree().try_get_matches_from(args)?;
        let request = request(&parsed)?;
        let context = context(&parsed)?;
        let receipt = generated::execute_tx_with_keys(
            &context,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await?
        .ok_or("receipt")?;
        assert!(receipt.success);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        let submissions = chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .clone();
        assert_eq!(submissions.len(), 1);
        let runtime = View::runtime(&client).await?;
        let decoded =
            rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
                .to_json();
        assert_eq!(decoded["era"][0], json!(64));
        assert_eq!(decoded["call"]["call_module"], json!("SubtensorModule"));
        let expected = match request.op {
            "set_weights" => rtao_runtime::calls::subtensor_module::set_mechanism_weights(
                NETUID,
                MECHID,
                vec![0, 1, 2],
                vec![21845, 43690, 65535],
                9,
            ),
            "reveal_weights" => rtao_runtime::calls::subtensor_module::reveal_weights(
                NETUID,
                vec![2, 0],
                vec![16384, 65535],
                vec![7, 8],
                9,
            ),
            "serve_axon" => rtao_runtime::calls::subtensor_module::serve_axon(
                NETUID,
                1,
                0xcb00_7109,
                8080,
                4,
                4,
                0,
                0,
            ),
            "serve_axon_tls" => rtao_runtime::calls::subtensor_module::serve_axon_tls(
                NETUID,
                1,
                0x2001_0db8_0000_0000_0000_0000_0000_0001,
                443,
                6,
                4,
                0,
                0,
                vec![1, 0xab],
            ),
            "reset_axon" => {
                rtao_runtime::calls::subtensor_module::serve_axon(NETUID, 0, 0, 1, 4, 4, 0, 0)
            }
            _ => return Err("unexpected request".into()),
        }
        .compose(&runtime)?;
        assert!(submissions[0].ends_with(expected.as_bytes()));
        client.close().await;
    }
    Ok(())
}

struct FixedClock;
impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs_f64(1_752_076_800.25)
    }
}

#[tokio::test]
async fn set_selects_chain_commit_mode_and_commit_forces_it_even_when_plaintext() -> TestResult {
    for (command, enabled) in [("set", true), ("commit", false)] {
        let keys = Keys::new()?;
        let (client, chain) = client(state(&keys, false, enabled)?).await?;
        let parsed = tree().try_get_matches_from([
            "rtao",
            "weights",
            command,
            "--netuid",
            "3",
            "--mechid",
            "1",
            "--uids",
            "0,1,2",
            "--weights",
            "1,2,3",
        ])?;
        let request = request(&parsed)?;
        let intent = request.intent()?;
        let intent = intent.as_dyn();
        let snapshot = client.at(BlockRef::Best).await?;
        let runtime = View::runtime(&snapshot).await?;
        let built = intent
            .build_dyn(&BuildCx {
                view: &snapshot as &dyn DynView,
                origin: OriginView::Account(keys.key.account()),
                runtime: &runtime,
                clock: &FixedClock,
                spec: intent.spec(),
            })
            .await?;
        let prefix = rtao_runtime::calls::subtensor_module::commit_timelocked_mechanism_weights(
            NETUID,
            MECHID,
            vec![],
            1,
            4,
        )
        .compose(&runtime)?;
        assert_eq!(&built.call.as_bytes()[..5], &prefix.as_bytes()[..5]);
        assert!(built.call.as_bytes().len() > 100);
        assert!(
            built.extras["reveal_round"]
                .as_u64()
                .is_some_and(|round| round > 0)
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn raw_null_gate_and_serving_validation_refuse_before_hotkey_unlock() -> TestResult {
    for (args, expected) in [
        (
            vec![
                "rtao",
                "weights",
                "set",
                "--netuid",
                "3",
                "--mechid",
                "1",
                "--uids",
                "0,1",
                "--weights",
                "1,2",
                "--raw-u16",
            ],
            "raw_u16 requires a subnet running Null consensus.",
        ),
        (
            vec![
                "rtao",
                "axon",
                "set",
                "--netuid",
                "3",
                "--ip",
                "invalid",
                "--port",
                "443",
                "--tls",
                "--certificate",
                "zz",
            ],
            "'invalid' does not appear to be an IPv4 or IPv6 address",
        ),
        (
            vec![
                "rtao",
                "axon",
                "set",
                "--netuid",
                "3",
                "--ip",
                "::1",
                "--port",
                "443",
                "--tls",
                "--certificate",
                "zz",
            ],
            "non-hexadecimal number found in fromhex() arg at position 0",
        ),
        (
            vec![
                "rtao",
                "weights",
                "reveal",
                "--netuid",
                "3",
                "--uids",
                "1",
                "--weights",
                "0",
                "--salt",
                "1",
            ],
            "All weights are zero; nothing to reveal.",
        ),
    ] {
        let keys = Keys::new()?;
        let (client, chain) = client(state(&keys, false, false)?).await?;
        let mut args = args;
        args.extend(["--json", "--yes", "--no-mev-shield"]);
        let parsed = tree().try_get_matches_from(args)?;
        let request = request(&parsed)?;
        let context = context(&parsed)?;
        let error = generated::execute_tx_with_keys(
            &context,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await
        .err()
        .ok_or("must refuse")?;
        assert!(error.message.contains(expected), "{}", error.message);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn raw_null_keeps_exact_integer_weights_and_registration_and_rate_checks_cannot_be_bypassed()
-> TestResult {
    let parsed = tree().try_get_matches_from([
        "rtao",
        "weights",
        "set",
        "--netuid",
        "3",
        "--mechid",
        "1",
        "--uids",
        "0,1,2",
        "--weights",
        "1,2,3",
        "--raw-u16",
        "--yes",
        "--json",
        "--no-mev-shield",
    ])?;
    for condition in ["valid", "unregistered", "permit", "rate-limited"] {
        let keys = Keys::new()?;
        let mut state = state(&keys, true, false)?;
        if condition == "unregistered" {
            let entry = state
                .runtime
                .storage_entry("SubtensorModule", "Uids")
                .ok_or("Uids storage")?;
            let key = state.runtime.storage_key(
                entry,
                &[
                    rtao::core::codec::Value::Int(i128::from(NETUID.0)),
                    rtao::core::codec::Value::Bytes(keys.key.account().0.to_vec()),
                ],
            )?;
            state.seed_encoded(key, None);
        } else if condition == "permit" {
            state.seed_typed(
                "SubtensorModule",
                "ValidatorPermit",
                (NETUID,),
                vec![false, false, false],
            )?;
        } else if condition == "rate-limited" {
            state.seed_typed(
                "SubtensorModule",
                "LastUpdate",
                (NetUidStorageIndex::new(NETUID, MECHID)?,),
                vec![0_u64, 0, 100],
            )?;
        }
        let (client, chain) = client(state).await?;
        let request = request(&parsed)?;
        let context = context(&parsed)?;
        let result = generated::execute_tx_with_keys(
            &context,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await;
        let submissions = chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .clone();
        if condition == "valid" {
            assert!(result?.ok_or("receipt")?.success);
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
            assert_eq!(submissions.len(), 1);
            let runtime = View::runtime(&client).await?;
            let expected = rtao_runtime::calls::subtensor_module::set_mechanism_weights(
                NETUID,
                MECHID,
                vec![0, 1, 2],
                vec![1, 2, 3],
                0,
            )
            .compose(&runtime)?;
            assert!(submissions[0].ends_with(expected.as_bytes()));
        } else {
            let error = result.err().ok_or("must refuse")?;
            assert!(error.message.contains(match condition {
                "unregistered" => "not registered",
                "permit" => "sole Null validator permit holder",
                _ => "rate limit",
            }));
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
            assert!(submissions.is_empty());
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn operator_dry_runs_use_semantic_review_without_unlock_or_submission() -> TestResult {
    for mut args in [
        vec![
            "rtao",
            "weights",
            "set",
            "--netuid",
            "3",
            "--mechid",
            "1",
            "--uids",
            "0,1,2",
            "--weights",
            "1,2,3",
        ],
        vec!["rtao", "axon", "reset", "--netuid", "3"],
    ] {
        args.extend(["--dry-run", "--json", "--no-mev-shield"]);
        let parsed = tree().try_get_matches_from(args)?;
        let keys = Keys::new()?;
        let (client, chain) = client(state(&keys, false, false)?).await?;
        let context = context(&parsed)?;
        let request = request(&parsed)?;
        let result = generated::execute_tx_with_keys(
            &context,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await?;
        assert!(result.is_none());
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
        client.close().await;
    }
    Ok(())
}
