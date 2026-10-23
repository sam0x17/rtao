//! Native socket + retained signed-pair recovery, without opening the public
//! journaled shield gate. Synthetic bodies/events model inner execution; this
//! does not test actual chain decryption or qualify any deployed runtime.
#![allow(clippy::expect_used)]
use super::*;
use crate::{ChainClock, ClockGuard, Metrics, WeightPolicy};
use rtao_client::journal::{
    AttemptOutcome, AttemptRole, ExecutionMode, JournalControl, PairPhase, ShieldPair,
    ShieldPolicy, SignedAttempt,
};
use rtao_client::{BlockRef, Client, Mortality, NoncePolicy, Period, SubmitOptions};
use rtao_runtime::calls::subtensor_module as calls;
use rtao_signer::{KeypairSigner, SignPurpose, Signer};
use serde_json::json;

async fn component(
    client: &Client,
    signer: &KeypairSigner,
    call: rtao_client::CallBytes,
    nonce: u64,
    id: u64,
    role: AttemptRole,
    material: &serde_json::Value,
) -> anyhow::Result<SignedAttempt> {
    let unsigned = client
        .prepare_call(
            call,
            signer,
            &SubmitOptions {
                nonce: NoncePolicy::Explicit(nonce),
                mortality: Mortality::Mortal(Period::new(8)),
                ..Default::default()
            },
        )
        .await?;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    let (bytes, hash) = client.attach_signature(&unsigned, signature).await?;
    Ok(SignedAttempt {
        id,
        role,
        material_generation: Some(1),
        hash,
        bytes: bytes.to_vec(),
        nonce,
        spec_version: unsigned.call().stamp().spec_version,
        transaction_version: unsigned.call().stamp().transaction_version,
        start: unsigned.submission_head(),
        expiry: unsigned.expiry(),
        era_anchor: unsigned.era_block_hash(),
        era: rtao_core::codec::value::to_corpus_json(unsigned.era()),
        metadata_hash: unsigned.metadata_hash(),
        material: material.clone(),
        next: unsigned.submission_head(),
        finalized: None,
        outcome: AttemptOutcome::Unresolved,
        evidence_conflicts: Vec::new(),
        extensions: Default::default(),
    })
}

#[tokio::test]
async fn native_paired_weight_lost_handoff_reopens_and_restores_one_reveal_while_paused()
-> anyhow::Result<()> {
    let state = super::super::tests::fixture_state()?;
    let node = rtao_testkit::FakeNode::with_state(Arc::new(std::sync::Mutex::new(state))).await?;
    let mut config =
        rtao_client::ClientConfig::new(rtao_client::Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let connect = || {
        rtao_rpc::Session::connect(rtao_rpc::SessionConfig::new(
            url::Url::parse(node.url()).expect("local fixture URL"),
        ))
    };
    let session = connect().await?;
    let client = Client::from_session(session.clone(), config.clone()).await?;
    let signer = KeypairSigner::from_seed(&[7; 32], rtao_primitives::CryptoType::Ed25519)?;
    let account = signer.account();
    let root = std::path::PathBuf::from(
        std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
    )
    .join(format!("native-paired-weight-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let path = root.join("journal.json");
    let make = || {
        WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )
    };
    let journal = OperationJournal::open(&path)?;
    let scheduler = make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), account)?;
    scheduler.enqueue(
        "paired-commit",
        Some("application-revision".into()),
        MechId(0),
        WeightsInput::Parallel {
            uids: vec![Uid(1)],
            weights: vec![1.0],
        },
    )?;
    let best = client.at(BlockRef::Best).await?;
    let preflight =
        WeightPreflight::fetch(&best, NetUid(1), MechId(0), super::super::tests::HOTKEY).await?;
    let commit = PreparedWeightCommit {
        ciphertext: vec![4, 5, 6],
        reveal_round: 500,
    };
    let binding = scheduler
        .prepare_binding(
            MechId(0),
            (vec![Uid(1)], vec![u16::MAX]),
            Some(commit.clone()),
            &preflight,
        )?
        .ok_or_else(|| anyhow::anyhow!("durable binding"))?;
    let material = binding.record()?.material;
    let owner = binding.reserve()?;
    let call = calls::commit_timelocked_mechanism_weights(
        NetUid(1),
        MechId(0),
        commit.ciphertext.clone(),
        commit.reveal_round,
        4,
    )
    .compose(&client.refresh_runtime().await?)?;
    let inner = component(
        &client,
        &signer,
        call,
        1,
        0,
        AttemptRole::Inner { pair: 0 },
        &material,
    )
    .await?;
    let ciphertext = rtao_client::ShieldCiphertext::seal(&vec![0; 1184], &inner.bytes).await?;
    let call = rtao_runtime::calls::mev_shield::submit_encrypted(ciphertext.into_bytes())
        .compose(&client.refresh_runtime().await?)?;
    let carrier = component(
        &client,
        &signer,
        call,
        0,
        1,
        AttemptRole::Carrier { pair: 0 },
        &material,
    )
    .await?;
    journal.update(|image| {
        let revision = image.revision + 1;
        let record = image
            .operations
            .get_mut(binding.id())
            .ok_or(rtao_client::journal::JournalError::Identity)?;
        record.mode = ExecutionMode::Shielded(ShieldPolicy {
            protocol: "synthetic-retained-pair-only".into(),
            period: 8,
            maximum_pairs: 1,
            extensions: Default::default(),
        });
        record.attempts = vec![inner.clone(), carrier.clone()];
        record.shield_pairs = vec![ShieldPair {
            id: 0,
            material_generation: 1,
            inner: 0,
            carrier: Some(1),
            phase: PairPhase::DispatchAuthorized { revision },
            extensions: Default::default(),
        }];
        Ok(())
    })?;
    // Observe the actual native wire and the already-published disk image.
    // Capture a path, not a journal owner that would defeat actual reopen.
    let disk = path.clone();
    let expected_carrier = format!("0x{}", hex::encode(&carrier.bytes));
    node.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let image: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&disk).expect("publication before author"),
            ).expect("durable JSON");
            assert_eq!(image["operations"]["paired-commit"]["attempts"].as_array().expect("attempts").len(), 2);
            assert_eq!(request.params[0], expected_carrier);
            let reply = state.dispatch(request);
            let number = state.block;
            super::super::tests::seed_at(state, number, "SubtensorModule", "TimelockedWeightCommits",
                (rtao_primitives::NetUidStorageIndex(1), 5u64),
                vec![(account, number, commit.ciphertext.clone(), commit.reveal_round)],
            ).expect("synthetic queue entry");
            state.set_finalized(number).expect("synthetic finalized head");
            reply
        });
        let encoded_inner = format!("0x{}", hex::encode(&inner.bytes));
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome
                && let Some(list) = raw["block"]["extrinsics"].as_array_mut()
                && !list.is_empty()
            {
                list.push(json!(encoded_inner));
                let events = (0..list.len()).map(|i| json!({"phase":"ApplyExtrinsic","extrinsic_idx":i,
                    "module_id":"System","event_id":"ExtrinsicSuccess",
                    "attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})).collect::<Vec<_>>();
                state.seed_events(request.params[0].as_str().expect("pinned hash"), json!(events)).expect("synthetic events");
            }
            reply
        });
    });
    let (custody, _) = rtao_rpc::AuthorOwnership::new(rtao_rpc::H256(carrier.hash.0), owner);
    let watch = session
        .submit_and_watch_owned(&carrier.bytes, custody)
        .await?;
    drop(watch);
    assert_eq!(scheduler.reveals.pending(), 0);
    // Simulate lost acknowledgment/handoff by discarding every client/journal
    // owner before reading history through a freshly connected native client.
    client.close().await;
    drop(session);
    drop(binding);
    drop(scheduler);
    drop(journal);
    let journal = OperationJournal::open(&path)?;
    journal.control(JournalControl {
        submissions_paused: true,
        allow_obligations: false,
        extensions: Default::default(),
    })?;
    let client = Client::from_session(connect().await?, config).await?;
    let mut scheduler =
        make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), account)?;
    let pinned = client.at(BlockRef::Finalized).await?.block().clone();
    let raw = node
        .mutate_state(|state| state.header(&format!("0x{}", hex::encode(pinned.hash.0))))
        .ok_or_else(|| anyhow::anyhow!("synthetic finalized header"))?;
    let head = rtao_client::BlockHeader::from_json(raw)?;
    for _ in 0..2 {
        scheduler
            .recover_finalized(&client, &head, &Metrics::default())
            .await?;
        assert_eq!(scheduler.reveals.pending(), 1);
        assert!(scheduler.pending_work(MechId(0))?.is_none());
    }
    let record = journal.bind("paired-commit")?.record()?;
    assert_eq!(
        record
            .finalized_effect()?
            .ok_or_else(|| anyhow::anyhow!("inner effect"))?
            .hash,
        inner.hash
    );
    assert_ne!(record.attempts.last().expect("carrier").hash, inner.hash);
    assert_eq!(
        record.intent.correlation_id.as_deref(),
        Some("application-revision")
    );
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "author_submitAndWatchExtrinsic")
            .count(),
        1
    );
    // The existing atomic checkpoint also preserves the watcher on a second
    // scheduler restart, rather than re-handing off the logical operation.
    drop(scheduler);
    let scheduler = make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), account)?;
    assert_eq!(scheduler.reveals.pending(), 1);
    drop(scheduler);
    drop(journal);
    client.close().await;
    std::fs::remove_dir_all(root)?;
    Ok(())
}
