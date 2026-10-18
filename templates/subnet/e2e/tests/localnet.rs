//! Run both shipped images against Scenario's registered, staked localnet subnet.

use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use rtao::{
    client::{BlockRef, Finality, View, ViewExt},
    core::{codec::Value, value_ext::ValueExt},
    primitives::{AccountId, CryptoType, MechId, NetUidStorageIndex, TaoBalance},
    signer::Signer,
    wallet::{RegenerateFrom, SecretString, Wallet, WriteOptions},
};
use rtao_runtime::{calls::admin_utils, storage::subtensor_module as st};
use rtao_testkit::{Localnet, LocalnetProfile, LocalnetSlot, Scenario};
use std::{fs, path::PathBuf, process::Command, time::Duration};
use tokio::time::{Instant, timeout};

fn compose(arguments: &[&str], env_file: Option<&std::path::Path>) -> Result<String> {
    let mut command = Command::new("docker");
    command.arg("compose");
    if let Some(file) = env_file {
        command.arg("--env-file").arg(file);
    }
    let output = command.args(arguments).output()?;
    ensure!(
        output.status.success(),
        "compose {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn account_matches(value: &Value, account: AccountId) -> bool {
    value.value_bytes().as_deref() == Some(account.as_bytes().as_slice())
        || value
            .as_str()
            .is_some_and(|value| value.parse::<AccountId>().ok() == Some(account))
}

async fn exercise(ln: &Localnet, root: PathBuf) -> Result<()> {
    ln.bootstrap().await?;
    let setup = Scenario::new(ln)
        .with_slot(LocalnetSlot::new(0)?)
        .owner()
        .subnet()
        .start()
        .validator()
        .miner()
        .stake(TaoBalance(1_000_000_000_000))
        .commit_reveal(true)
        .weights_version(0)
        .build()
        .await?;
    let netuid = setup.netuid.context("scenario did not register a subnet")?;
    for call in [
        admin_utils::sudo_set_immunity_period(netuid, 0),
        admin_utils::sudo_set_commit_reveal_weights_interval(netuid, 1),
    ] {
        let report = ln
            .submit_admin(
                call.compose(&ln.client().runtime().await?)?,
                &setup.slot.owner,
            )
            .await?;
        ensure!(report.success, "subnet configuration failed: {report:?}");
    }
    let validator = setup.validator.context("validator was not registered")?;
    let miner = setup.miner.context("miner was not registered")?;
    let validator_uid = ln
        .client()
        .storage(&st::UIDS, (netuid, validator.account()))
        .await?
        .context("validator UID is absent")?;
    let miner_uid = ln
        .client()
        .storage(&st::UIDS, (netuid, miner.account()))
        .await?
        .context("miner UID is absent")?;
    // The miner's production policy admits only a registered validator permit.
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut heads = ln.client().blocks(Finality::Finalized)?;
    loop {
        let permits = ln
            .client()
            .storage(&st::VALIDATOR_PERMIT, (netuid,))
            .await?;
        if permits.get(usize::from(validator_uid)) == Some(&true) {
            break;
        }
        timeout(
            deadline.saturating_duration_since(Instant::now()),
            heads.next(),
        )
        .await?
        .context("finalized heads ended before permit")??;
    }
    let wallets = root.join("wallets");
    let state = root.join("state");
    for (role, signer, suffix) in [("validator", &validator, "hot"), ("miner", &miner, "miner")] {
        let wallet = Wallet::new("development", role, &wallets)?;
        let account = wallet
            .regenerate_hotkey(
                RegenerateFrom::Uri(SecretString::new(format!("//rtao-e2e//slot/0//{suffix}"))),
                CryptoType::Sr25519,
                WriteOptions::hotkey(),
            )
            .await?;
        ensure!(account == signer.account(), "development hotkey mismatch");
        fs::create_dir_all(state.join(role))?;
    }
    let env_file = root.join("compose.env");
    fs::write(
        &env_file,
        format!(
            "NETUID={}\nTEMPLATE_WALLET_ROOT={}\nTEMPLATE_STATE_ROOT={}\n",
            netuid.0,
            wallets.display(),
            state.display()
        ),
    )?;
    let mut cursor = ln.client().at(BlockRef::Finalized).await?.block().number;
    compose(
        &["--profile", "neurons", "up", "-d", "miner", "validator"],
        Some(&env_file),
    )?;
    let key = NetUidStorageIndex::new(netuid, MechId(0))?;
    let mut commit = None;
    let mut reveal = None;
    // Replay every finalized height. A fast chain may advance several heights
    // while one RPC completes; stream notifications alone can miss an event.
    while reveal.is_none() {
        let head = timeout(
            deadline.saturating_duration_since(Instant::now()),
            heads.next(),
        )
        .await?
        .context("finalized heads ended before reveal")??;
        if head.number <= cursor {
            continue;
        }
        for number in (cursor + 1)..=head.number {
            let snapshot = ln.client().at(BlockRef::Number(number)).await?;
            for event in snapshot.events().await?.records() {
                let (Value::List(fields) | Value::Tuple(fields)) = &event.attributes else {
                    continue;
                };
                let (who, index, is_commit) = if event
                    .matches("SubtensorModule", "TimelockedWeightsCommitted")
                    && fields.len() == 4
                {
                    (&fields[0], &fields[1], true)
                } else if event.matches("SubtensorModule", "TimelockedWeightsRevealed")
                    && fields.len() == 2
                {
                    (&fields[1], &fields[0], false)
                } else {
                    continue;
                };
                if account_matches(who, validator.account())
                    && index.as_u128() == Some(u128::from(key.0))
                {
                    if is_commit {
                        commit.get_or_insert(number);
                    } else {
                        ensure!(commit.is_some(), "reveal was observed without a commitment");
                        reveal = Some(number);
                    }
                }
            }
        }
        cursor = head.number;
    }
    let weights = ln
        .client()
        .storage(&st::WEIGHTS, (key, validator_uid))
        .await?;
    ensure!(
        weights == vec![(miner_uid, u16::MAX)],
        "revealed row does not reward the echo miner: {weights:?}"
    );
    let logs = compose(
        &["logs", "--no-color", "--tail", "1000", "validator"],
        Some(&env_file),
    )?;
    let marker = format!(
        "btauth echo accepted uid={miner_uid} hotkey={}",
        miner.account()
    );
    ensure!(
        logs.contains(&marker),
        "validator did not receive the authenticated echo"
    );
    fs::write(root.join("validator.log"), logs)?;
    compose(
        &["stop", "--timeout", "90", "validator", "miner"],
        Some(&env_file),
    )?;
    for role in ["validator", "miner"] {
        let id = compose(&["ps", "--all", "--quiet", role], Some(&env_file))?;
        let id = id.trim();
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "compose returned no exact {role} container identity"
        );
        let status = Command::new("docker")
            .args(["inspect", "--format", "{{.State.ExitCode}}", id])
            .output()?;
        ensure!(
            status.status.success() && String::from_utf8_lossy(&status.stdout).trim() == "0",
            "{role} did not shut down cleanly"
        );
    }
    let checkpoint: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("miner/state.json"))?)?;
    ensure!(
        checkpoint["state"]
            .as_u64()
            .is_some_and(|requests| requests > 0),
        "miner did not persist any authenticated request"
    );
    fs::write(
        root.join("template-result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1, "netuid": netuid.0,
            "validator": validator.account().to_string(), "miner": miner.account().to_string(),
            "validator_uid": validator_uid, "miner_uid": miner_uid,
            "commit_block": commit, "reveal_block": reveal,
            "authenticated_echo": true, "weights": weights,
            "miner_authenticated_requests": checkpoint["state"]
        }))?,
    )?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let root =
        PathBuf::from(std::env::var_os("RTAO_TEMP_ROOT").context("RTAO_TEMP_ROOT is required")?);
    ensure!(root.is_absolute(), "task root must be absolute");
    fs::create_dir_all(&root)?;
    // The CI owns this compose node. Never start or submit to a public endpoint.
    let deadline = Instant::now() + Duration::from_secs(180);
    let ln = loop {
        match Localnet::connect("ws://127.0.0.1:9944", LocalnetProfile::Fast).await {
            Ok(ln) => break ln,
            Err(error) if Instant::now() >= deadline => return Err(error.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    };
    let result = exercise(&ln, root).await;
    ln.client().close().await;
    result
}
