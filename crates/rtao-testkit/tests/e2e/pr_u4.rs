//! PR-U4 cross-decryption and future-round localnet reveal proof.

#[path = "../../../../ci/pr-u4-probe/deterministic.rs"]
mod deterministic;

use parity_scale_codec::{Decode, Encode};
use rtao_client::{BlockRef, SubmitOptions, View, ViewExt, Wait};
use rtao_core::{
    codec::Value,
    timelock::{WeightsTlockPayload, epoch_schedule::EpochScheduleState},
    value_ext::ValueExt,
};
use rtao_primitives::{AccountId, MechId, NetUidStorageIndex, TaoBalance};
use rtao_runtime::{
    calls::{admin_utils, subtensor_module},
    storage::subtensor_module as st,
};
use rtao_signer::Signer;
use rtao_testkit::{Localnet, LocalnetProfile, Scenario};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};

type ProofResult<T = ()> = Result<T, Box<dyn Error>>;

#[derive(Serialize)]
struct Request {
    reveal_round: u64,
    hotkey_hex: String,
    uids: Vec<u16>,
    values: Vec<u16>,
    version_key: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Vector {
    format: u8,
    bls_source: String,
    tle_commit: String,
    reveal_round: u64,
    hotkey_hex: String,
    uids: Vec<u16>,
    values: Vec<u16>,
    version_key: u64,
    ephemeral_key_hex: String,
    rng_seed_hex: String,
    payload_hex: String,
    ciphertext_hex: String,
    envelope_hex: String,
    registry_decrypted_payload_hex: Option<String>,
}

fn task_root() -> ProofResult<PathBuf> {
    let root = PathBuf::from(std::env::var("RTAO_TEMP_ROOT")?).canonicalize()?;
    if !root.is_absolute() || root.parent().is_none() {
        return Err("PR-U4 needs an absolute task-scoped temporary root".into());
    }
    Ok(root)
}

fn source_proof(root: &Path) -> ProofResult<PathBuf> {
    let prepared: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("pr-u4-proof/prepared.json"))?)?;
    if prepared["format"] != 1 || prepared["core_commit"] != rtao_core::BITTENSOR_CORE_COMMIT {
        return Err("PR-U4 probe and workspace must use the same pinned core commit".into());
    }
    let metadata_path = PathBuf::from(
        prepared["metadata"]
            .as_str()
            .ok_or("PR-U4 preparation omitted metadata")?,
    )
    .canonicalize()?;
    let binary = PathBuf::from(
        prepared["binary"]
            .as_str()
            .ok_or("PR-U4 preparation omitted binary")?,
    )
    .canonicalize()?;
    if !metadata_path.starts_with(root) || !binary.starts_with(root) {
        return Err(
            "PR-U4 binary and source evidence must belong to the recorded task root".into(),
        );
    }
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(metadata_path)?)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("PR-U4 metadata omitted packages")?;
    let bls: Vec<_> = packages.iter().filter(|p| p["name"] == "w3f-bls").collect();
    if bls.len() != 1
        || bls[0]["version"] != "0.1.3"
        || !bls[0]["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("registry+"))
    {
        return Err("PR-U4 probe must contain exactly one crates.io w3f-bls 0.1.3".into());
    }
    Ok(binary)
}

async fn run_probe(request: &Request, label: &str) -> ProofResult<Vector> {
    let root = task_root()?;
    let binary = source_proof(&root)?;
    let output = root.join("pr-u4-proof/records");
    fs::create_dir_all(&output)?;
    let label = format!("{label}-{}", std::process::id());
    let input_path = output.join(format!("{label}-request.json"));
    let output_path = output.join(format!("{label}-vector.json"));
    fs::write(&input_path, serde_json::to_vec_pretty(request)?)?;
    let mut command = Command::new(binary);
    command.arg(&input_path).arg(&output_path);
    if request.reveal_round == 1 {
        command.arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../vendor/recorded/drand/quicknet-1.json"),
        );
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(format!("PR-U4 probe exited with {status}").into()),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
            Ok(None) if Instant::now() >= deadline => {
                child.kill()?;
                child.wait()?;
                return Err("PR-U4 probe exceeded its 30-second deadline".into());
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let vector: Vector = serde_json::from_slice(&fs::read(output_path)?)?;
    assert_eq!(vector.format, 1);
    assert_eq!(vector.bls_source, "registry+w3f-bls=0.1.3");
    assert_eq!(
        vector.tle_commit,
        "5416406cfd32799e31e1795393d4916894de4468"
    );
    assert_eq!(vector.reveal_round, request.reveal_round);
    assert_eq!(vector.hotkey_hex, request.hotkey_hex);
    assert_eq!(vector.uids, request.uids);
    assert_eq!(vector.values, request.values);
    assert_eq!(vector.version_key, request.version_key);
    assert_eq!(
        vector.ephemeral_key_hex,
        hex::encode(deterministic::EPHEMERAL_KEY)
    );
    assert_eq!(vector.rng_seed_hex, hex::encode(deterministic::RNG_SEED));
    let payload = WeightsTlockPayload {
        hotkey: hex::decode(&request.hotkey_hex)?,
        uids: request.uids.clone(),
        values: request.values.clone(),
        version_key: request.version_key,
    }
    .encode();
    assert_eq!(hex::decode(&vector.payload_hex)?, payload);
    if request.reveal_round == 1 {
        assert_eq!(
            vector.registry_decrypted_payload_hex.as_deref(),
            Some(vector.payload_hex.as_str()),
            "registry BLS must independently decrypt the past-round payload"
        );
    }
    assert_eq!(
        hex::decode(&vector.ciphertext_hex)?,
        deterministic::encrypt(&payload, request.reveal_round)?,
        "registry and workspace BLS ciphertext bytes differ"
    );
    let envelope = hex::decode(&vector.envelope_hex)?;
    assert_eq!(
        rtao_core::timelock::reveal_round(&envelope)?,
        request.reveal_round
    );
    assert_eq!(
        rtao_core::timelock::inner_ciphertext(&envelope)?,
        hex::decode(&vector.ciphertext_hex)?
    );
    Ok(vector)
}

#[tokio::test]
async fn pr_u4_past_round_cross_decrypts_and_has_identical_ciphertext_bytes() -> ProofResult {
    let request = Request {
        reveal_round: 1,
        hotkey_hex: hex::encode([7u8; 32]),
        uids: vec![0, 2, 9],
        values: vec![17, u16::MAX, 0],
        version_key: 11,
    };
    let vector = run_probe(&request, "past-quicknet-1").await?;
    let signature_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/recorded/drand/quicknet-1.json");
    let signature: serde_json::Value = serde_json::from_slice(&fs::read(signature_path)?)?;
    assert_eq!(signature["round"], json!(request.reveal_round));
    let signature_hex = signature["signature"]
        .as_str()
        .ok_or("recorded drand response omitted signature")?;
    let envelope = hex::decode(vector.envelope_hex)?;
    let opened = rtao_core::timelock::decrypt_with_signature(&envelope, signature_hex)?;
    assert_eq!(opened, hex::decode(vector.payload_hex)?);
    let mut input = opened.as_slice();
    let decoded = WeightsTlockPayload::decode(&mut input)?;
    assert!(input.is_empty());
    assert_eq!(decoded.hotkey, [7u8; 32]);
    assert_eq!(decoded.uids, request.uids);
    assert_eq!(decoded.values, request.values);
    assert_eq!(decoded.version_key, request.version_key);
    assert!(rtao_core::timelock::decrypt_with_signature(&envelope, &"00".repeat(48)).is_err());
    Ok(())
}

fn reveal_matches(attributes: &Value, index: NetUidStorageIndex, hotkey: AccountId) -> bool {
    let pair = match attributes {
        Value::List(fields) | Value::Tuple(fields) if fields.len() == 2 => {
            Some((&fields[0], &fields[1]))
        }
        Value::Dict(_) => attributes
            .field("netuid_index")
            .or_else(|| attributes.field("netuid"))
            .zip(attributes.field("who")),
        _ => None,
    };
    pair.is_some_and(|(netuid, who)| {
        netuid.as_u128() == Some(u128::from(index.0))
            && (who.value_bytes().as_deref() == Some(hotkey.0.as_slice())
                || who
                    .as_str()
                    .is_some_and(|address| address == hotkey.to_string()))
    })
}

/// Submit crates.io-BLS ciphertext and observe its matching pallet reveal.
pub async fn scenario_16_pr_u4_localnet_reveal() -> ProofResult {
    if std::env::consts::OS != "linux"
        || std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_OS").as_deref() != Ok("Linux")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("scenario 16 runs on the hosted Ubuntu localnet lane".into());
    }
    let root = task_root()?;
    source_proof(&root)?;
    let endpoint = std::env::var("E2E_ENDPOINT")?;
    let profile = LocalnetProfile::parse(&std::env::var("E2E_BLOCKS")?)?;
    let localnet = Localnet::connect(&endpoint, profile).await?;
    let scenario = Scenario::new(&localnet)
        .owner()
        .subnet()
        .start()
        .validator()
        .stake(TaoBalance(1_000_000_000_000))
        .commit_reveal(true)
        .build()
        .await?;
    let netuid = scenario.netuid.ok_or("PR-U4 setup omitted subnet")?;
    let validator = scenario
        .validator
        .as_ref()
        .ok_or("PR-U4 setup omitted validator")?;
    let configured = localnet
        .client()
        .submit(
            admin_utils::sudo_set_commit_reveal_weights_interval(netuid, 1)
                .compose(&localnet.client().runtime().await?)?,
            &scenario.slot.owner,
            &SubmitOptions {
                wait: Wait::Inclusion,
                deadline: Some(Duration::from_secs(240)),
                ..Default::default()
            },
        )
        .await?;
    assert!(
        configured.success,
        "PR-U4 reveal-period setup failed: {configured:?}"
    );
    let validator_uid = localnet
        .client()
        .storage(&st::UIDS, (netuid, validator.account()))
        .await?
        .ok_or("PR-U4 validator has no UID")?;
    let permit_deadline = Instant::now() + Duration::from_secs(420);
    loop {
        let permits = localnet
            .client()
            .storage(&st::VALIDATOR_PERMIT, (netuid,))
            .await?;
        if permits.get(usize::from(validator_uid)) == Some(&true) {
            break;
        }
        if Instant::now() >= permit_deadline {
            return Err("PR-U4 validator did not receive a permit after staking".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let snapshot = localnet.client().at(BlockRef::Best).await?;
    let (
        (
            tempo,
            last_epoch_block,
            pending_epoch_at,
            subnet_epoch_index,
            blocks_since_last_step,
            period,
            version_key,
            cr_version,
        ),
        latest,
        uid,
        duration,
    ) = tokio::try_join!(
        snapshot.fetch((
            st::TEMPO.at((netuid,)),
            st::LAST_EPOCH_BLOCK.at((netuid,)),
            st::PENDING_EPOCH_AT.at((netuid,)),
            st::SUBNET_EPOCH_INDEX.at((netuid,)),
            st::BLOCKS_SINCE_LAST_STEP.at((netuid,)),
            st::REVEAL_PERIOD_EPOCHS.at((netuid,)),
            st::WEIGHTS_VERSION_KEY.at((netuid,)),
            st::COMMIT_REVEAL_WEIGHTS_VERSION.at(()),
        )),
        snapshot.storage(&rtao_runtime::storage::drand::LAST_STORED_ROUND, ()),
        snapshot.storage(&st::UIDS, (netuid, validator.account())),
        snapshot.block_time(),
    )?;
    let uid = uid.ok_or("PR-U4 validator has no UID")?;
    let schedule = EpochScheduleState {
        tempo,
        last_epoch_block,
        pending_epoch_at,
        subnet_epoch_index,
        blocks_since_last_step,
        current_block: snapshot.block().number,
    };
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
    let clock_round = now
        .as_secs()
        .saturating_sub(rtao_core::timelock::constants::GENESIS_TIME)
        / rtao_core::timelock::constants::DRAND_PERIOD;
    let future_clock_round = clock_round.checked_add(3).ok_or("drand round overflow")?;
    let round = rtao_core::timelock::timelock_commit::reveal_round_at(
        now.as_secs_f64(),
        &schedule,
        period,
        duration.as_secs_f64(),
    )?
    .max(latest.checked_add(2).ok_or("drand round overflow")?)
    .max(future_clock_round);
    assert!(
        round > latest,
        "PR-U4 commit must target an unpublished on-chain round"
    );
    let request = Request {
        reveal_round: round,
        hotkey_hex: hex::encode(validator.account().0),
        uids: vec![uid],
        values: vec![u16::MAX],
        version_key,
    };
    let label = format!("future-slot-{}", scenario.slot.index);
    let vector = run_probe(&request, &label).await?;
    let before_submission = localnet
        .client()
        .storage(&rtao_runtime::storage::drand::LAST_STORED_ROUND, ())
        .await?;
    assert!(
        round > before_submission,
        "PR-U4 round was already ingested before the commit was submitted"
    );
    let reveal_timestamp = round
        .checked_sub(1)
        .and_then(|n| n.checked_mul(rtao_core::timelock::constants::DRAND_PERIOD))
        .and_then(|n| n.checked_add(rtao_core::timelock::constants::GENESIS_TIME))
        .ok_or("drand reveal timestamp overflow")?;
    assert!(
        reveal_timestamp
            > SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs(),
        "PR-U4 round was already due before the commit was submitted"
    );
    let report = localnet
        .client()
        .submit(
            subtensor_module::commit_timelocked_weights(
                netuid,
                hex::decode(vector.ciphertext_hex)?,
                round,
                cr_version,
            )
            .compose(&localnet.client().runtime().await?)?,
            validator,
            &SubmitOptions {
                wait: Wait::Inclusion,
                deadline: Some(Duration::from_secs(240)),
                ..Default::default()
            },
        )
        .await?;
    assert!(report.success, "PR-U4 commit failed: {report:?}");
    let inclusion = report
        .block_number
        .ok_or("PR-U4 receipt omitted inclusion height")?;
    let index = NetUidStorageIndex::new(netuid, MechId(0))?;
    let deadline = Instant::now() + Duration::from_secs(420);
    let mut cursor = inclusion;
    loop {
        let head = localnet.client().at(BlockRef::Best).await?;
        while cursor <= head.block().number {
            let at = localnet.client().at(BlockRef::Number(cursor)).await?;
            let events = at.events().await?;
            if events.records().iter().any(|event| {
                event.matches("SubtensorModule", "TimelockedWeightsRevealed")
                    && reveal_matches(&event.attributes, index, validator.account())
            }) {
                assert_eq!(
                    at.storage(&st::WEIGHTS, (index, uid)).await?,
                    vec![(uid, u16::MAX)]
                );
                fs::write(
                    root.join(format!("pr-u4-proof/records/{label}-reveal.json")),
                    serde_json::to_vec_pretty(&json!({
                        "format":1,"netuid":netuid.0,"hotkey":validator.account().to_string(),"reveal_round":round,
                    "inclusion_block":inclusion,"reveal_block":cursor,"reveal_hash":format!("0x{}",hex::encode(at.block().hash.0)),
                    "preflight_drand_round":latest,"pre_submission_drand_round":before_submission,"round_timestamp":reveal_timestamp,
                        "event":"TimelockedWeightsRevealed","uids":[uid],"values":[u16::MAX],"bls_source":"registry+w3f-bls=0.1.3"
                    }))?,
                )?;
                localnet.client().close().await;
                return Ok(());
            }
            cursor = cursor
                .checked_add(1)
                .ok_or("PR-U4 observation height overflow")?;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "PR-U4 future-round commit had no matching reveal through block {}",
                head.block().number
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
