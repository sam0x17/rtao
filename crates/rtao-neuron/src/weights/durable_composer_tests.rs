//! Weight call composition beyond the compiled descriptors (D-088): each
//! attempt wraps only its own call, benign shape changes compose by name,
//! real incompatibilities pause without signing, and an application encoder's
//! bytes are validated before signing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::super::{WeightAttemptOutcome, WeightCall, WeightCallEncoder};
use super::public_tests::{
    advance, close_cr_lab, cr_lab_with, module_error, plain_author, scored, until_authored,
};
use super::*;
use crate::{ChainClock, ClockGuard, WeightPolicy};
use frame_metadata::v15::RuntimeMetadataV15;
use rtao_client::Client;
use rtao_client::journal::Incompatibility;
use rtao_client::{ShieldPeriod, UnsignedExtrinsic};
use rtao_core::codec::Value;
use rtao_primitives::IntoCoreValue;
use rtao_primitives::NetUidStorageIndex;
use rtao_runtime::RuntimeHandle;
use scale_info::{Field, TypeDef, TypeDefPrimitive, form::PortableForm};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

const COMMIT_CALL: &str = "commit_timelocked_mechanism_weights";
const SET_CALL: &str = "set_mechanism_weights";

fn primitive(meta: &RuntimeMetadataV15, wanted: TypeDefPrimitive) -> u32 {
    meta.types
        .types
        .iter()
        .find(|ty| matches!(&ty.ty.type_def, TypeDef::Primitive(p) if *p == wanted))
        .map(|ty| ty.id)
        .expect("primitive")
}

fn call_fields<'a>(
    meta: &'a mut RuntimeMetadataV15,
    call: &str,
) -> &'a mut Vec<Field<PortableForm>> {
    let calls = meta
        .pallets
        .iter()
        .find(|pallet| pallet.name == "SubtensorModule")
        .and_then(|pallet| pallet.calls.as_ref())
        .expect("calls")
        .ty
        .id;
    let ty = meta
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == calls)
        .expect("call enum");
    let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
        panic!("call enum")
    };
    &mut variants
        .variants
        .iter_mut()
        .find(|variant| variant.name == call)
        .expect("call")
        .fields
}

fn field_type(meta: &mut RuntimeMetadataV15, call: &str, name: &str) -> u32 {
    call_fields(meta, call)
        .iter()
        .find(|field| field.name.as_deref() == Some(name))
        .expect("field")
        .ty
        .id
}

fn retype(meta: &mut RuntimeMetadataV15, call: &str, name: &str, ty: u32) {
    call_fields(meta, call)
        .iter_mut()
        .find(|field| field.name.as_deref() == Some(name))
        .expect("field")
        .ty = ty.into();
}

/// `commit` as a plain `Vec<u8>` instead of `BoundedVec<u8, _>`: a new shape
/// hash with identical encoding.
fn unbounded_commit(meta: &mut RuntimeMetadataV15) {
    let bounded = field_type(meta, COMMIT_CALL, "commit");
    let inner = meta
        .types
        .types
        .iter()
        .find(|ty| ty.id == bounded)
        .and_then(|ty| match &ty.ty.type_def {
            TypeDef::Composite(composite) => composite.fields.first().map(|f| f.ty.id),
            _ => None,
        })
        .expect("BoundedVec newtype");
    retype(meta, COMMIT_CALL, "commit", inner);
}

/// A new leading `extra: u64` argument, which name resolution refuses.
fn extra_field(call: &'static str) -> impl FnOnce(&mut RuntimeMetadataV15) {
    move |meta| {
        let u64 = primitive(meta, TypeDefPrimitive::U64);
        call_fields(meta, call).insert(
            0,
            Field {
                name: Some("extra".into()),
                ty: u64.into(),
                type_name: Some("u64".into()),
                docs: vec![],
            },
        );
    }
}

/// `version_key` narrowed to the netuid type, which refuses even by name.
fn narrowed_set_call(meta: &mut RuntimeMetadataV15) {
    let netuid = field_type(meta, SET_CALL, "netuid");
    retype(meta, SET_CALL, "version_key", netuid);
}

fn patched(
    edit: impl FnOnce(&mut RuntimeMetadataV15),
) -> impl FnOnce(&mut rtao_testkit::FakeState) -> anyhow::Result<()> {
    move |state| Ok(state.patch_metadata(edit)?)
}

fn scheduler() -> anyhow::Result<WeightScheduler> {
    WeightScheduler::new(
        WeightPolicy::default(),
        Arc::new(ChainClock::new(1_800_000_000_000)),
        ClockGuard::default(),
    )
}

/// Encodes every live field of the requested call by name through the core,
/// supplying zero for a field the scheduler does not prepare.
struct ByNameEncoder {
    contract: &'static str,
    calls: AtomicUsize,
}
impl ByNameEncoder {
    fn new(contract: &'static str) -> Arc<Self> {
        Arc::new(Self {
            contract,
            calls: AtomicUsize::new(0),
        })
    }
}
impl WeightCallEncoder for ByNameEncoder {
    fn contract(&self) -> &str {
        self.contract
    }
    fn encode(
        &self,
        runtime: &RuntimeHandle,
        call: &WeightCall<'_>,
    ) -> Result<Vec<u8>, Incompatibility> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (name, known): (&str, Vec<(&str, Value)>) = match *call {
            WeightCall::Set {
                netuid,
                mechanism,
                dests,
                weights,
                version_key,
            } => (
                SET_CALL,
                vec![
                    ("netuid", netuid.into_core_value()),
                    ("mecid", mechanism.into_core_value()),
                    ("dests", dests.to_vec().into_core_value()),
                    ("weights", weights.to_vec().into_core_value()),
                    ("version_key", version_key.into_core_value()),
                ],
            ),
            WeightCall::Commit {
                netuid,
                mechanism,
                commit,
                reveal_round,
                commit_reveal_version,
            } => (
                COMMIT_CALL,
                vec![
                    ("netuid", netuid.into_core_value()),
                    ("mecid", mechanism.into_core_value()),
                    ("commit", Value::Bytes(commit.to_vec())),
                    ("reveal_round", reveal_round.into_core_value()),
                    (
                        "commit_reveal_version",
                        commit_reveal_version.into_core_value(),
                    ),
                ],
            ),
        };
        let mut params = known
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<Vec<_>>();
        params.push(("extra".to_owned(), Value::Int(0)));
        runtime
            .core()
            .compose_call("SubtensorModule", name, &Value::record(params))
            .map_err(|error| Incompatibility {
                code: "test-encoder".into(),
                descriptor: None,
                detail: error.to_string(),
                extensions: Default::default(),
            })
    }
}

/// Records each semantic check and accepts it.
#[derive(Default)]
struct CountingGuard(AtomicUsize);
impl rtao_client::journal::SemanticGuard for CountingGuard {
    fn check<'a>(
        &'a self,
        _: &'a Client,
        _: &'a rtao_client::journal::OperationIntent,
        _: &'a UnsignedExtrinsic,
    ) -> rtao_signer::BoxFuture<'a, Result<(), Incompatibility>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn outcome(scheduler: &WeightScheduler) -> WeightAttemptOutcome {
    scheduler
        .last_attempt(MechId(0))
        .map(|attempt| attempt.outcome.clone())
        .expect("attempt")
}

/// A changed `set_mechanism_weights` shape used to refuse every timelocked
/// commit in the executor's surface review, after the cadence slot was
/// reserved. Each intent now wraps only its own call.
#[tokio::test]
async fn a_changed_direct_row_call_never_refuses_timelocked_commits() -> anyhow::Result<()> {
    let (lab, cx, account) = cr_lab_with("composer-set", 4, patched(narrowed_set_call)).await?;
    let mut scheduler = scheduler()?;
    let tails = plain_author(&lab, account, |_, _| {});
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    assert!(
        matches!(outcome(&scheduler), WeightAttemptOutcome::Submitted { .. }),
        "{:?}",
        outcome(&scheduler)
    );
    assert_eq!(tails.lock().unwrap()[0].2, 4);
    let rendered = cx.metrics.render();
    assert!(
        !rendered.contains("rtao_weights_errors_total"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_resolved_composition_total"),
        "the commit call itself is unchanged and compiled: {rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A shape change that cannot alter the encoding submits through
/// name-resolved composition, with a metric and no pause.
#[tokio::test]
async fn a_benign_commit_shape_change_submits_by_name() -> anyhow::Result<()> {
    let (lab, cx, account) = cr_lab_with("composer-benign", 4, patched(unbounded_commit)).await?;
    let mut scheduler = scheduler()?;
    let tails = plain_author(&lab, account, |_, _| {});
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    assert!(
        matches!(outcome(&scheduler), WeightAttemptOutcome::Submitted { .. }),
        "{:?}",
        outcome(&scheduler)
    );
    assert_eq!(tails.lock().unwrap()[0].2, 4);
    assert_eq!(scheduler.reveals.pending(), 1);
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_resolved_composition_total 1"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_surface_paused_total"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_errors_total"),
        "{rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A real incompatibility pauses the mechanism per head without scoring,
/// signing or counting an error.
#[tokio::test]
async fn an_incompatible_commit_call_pauses_without_signing() -> anyhow::Result<()> {
    let (lab, cx, _) = cr_lab_with("composer-paused", 4, patched(extra_field(COMMIT_CALL))).await?;
    let mut scheduler = scheduler()?;
    for _ in 0..3 {
        let head = advance(&lab)?;
        Box::pin(
            scheduler.on_finalized(&cx, &head, |_| panic!("a paused mechanism must not score")),
        )
        .await?;
    }
    assert_eq!(lab.authors(), 0);
    let WeightAttemptOutcome::PausedSurface { descriptor, detail } = outcome(&scheduler) else {
        panic!("expected a pause: {:?}", outcome(&scheduler))
    };
    assert_eq!(descriptor, format!("call:SubtensorModule.{COMMIT_CALL}"));
    assert!(detail.contains("field extra"), "{detail}");
    assert!(detail.contains("WeightCallEncoder"), "{detail}");
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_surface_paused_total 3"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_errors_total"),
        "{rendered}"
    );
    assert!(!cx.metrics.readiness().runtime_verified);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// The application encoder composes a call name resolution refuses; the
/// journaled shielded submission keeps the semantic guard and records the
/// encoder contract with each attempt it composed, never in the operation's
/// identity. Retained work with such an attempt needs that contract again.
#[tokio::test]
async fn an_application_encoder_composes_a_changed_call_through_the_journal() -> anyhow::Result<()>
{
    let (lab, cx, account) =
        cr_lab_with("composer-encoder", 4, patched(extra_field(COMMIT_CALL))).await?;
    let encoder = ByNameEncoder::new("test-encoder-v1");
    let guard = Arc::new(CountingGuard::default());
    let mut scheduler = scheduler()?
        .with_journal(
            lab.journal().unwrap().clone(),
            lab.client.genesis(),
            NetUid(1),
            account,
        )?
        .with_semantic_guard("test-semantics-v1", guard.clone())
        .with_shield_protocol(lab.protocol.clone())
        .with_shielded_submission(ShieldPeriod::P4)
        .with_call_encoder(encoder.clone());
    let signed = Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
    let captured = signed.clone();
    lab.after_inclusion(move |state, record, number| {
        let pair = record.shield_pairs.last().expect("shield pair");
        let inner = record.attempt(pair.inner).expect("inner attempt");
        let (ciphertext, round): (Vec<u8>, u64) =
            serde_json::from_value(record.material["commit"].clone()).expect("commit material");
        *captured.lock().unwrap() = Some(inner.bytes.clone());
        super::super::tests::seed_at(
            state,
            number,
            "SubtensorModule",
            "TimelockedWeightCommits",
            (NetUidStorageIndex(1), 5u64),
            vec![(account, number, ciphertext, round)],
        )
        .expect("native queue model");
    });
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    assert!(
        matches!(outcome(&scheduler), WeightAttemptOutcome::Submitted { .. }),
        "{:?}",
        outcome(&scheduler)
    );
    assert!(encoder.calls.load(Ordering::SeqCst) >= 1);
    assert!(guard.0.load(Ordering::SeqCst) >= 1, "semantic guard ran");
    let snapshot = lab.journal().unwrap().snapshot()?;
    let record = snapshot.operations.values().next().expect("operation");
    assert!(
        record.intent.effects.get("call_encoder").is_none(),
        "{}",
        record.intent.effects
    );
    assert!(!record.attempts.is_empty());
    for attempt in &record.attempts {
        assert_eq!(attempt.material["call_encoder"], json!("test-encoder-v1"));
    }
    assert_eq!(
        super::signed_encoder_contract(record)?.as_deref(),
        Some("test-encoder-v1")
    );
    let binding = lab.journal().unwrap().bind(&record.intent.id)?;
    for installed in [None, Some("test-encoder-v2")] {
        let mut other = self::scheduler()?;
        if let Some(contract) = installed {
            other = other.with_call_encoder(ByNameEncoder::new(contract));
        }
        assert_eq!(
            other.work_encoder(Some(&binding))?.err().as_deref(),
            Some("test-encoder-v1")
        );
    }
    let same = self::scheduler()?.with_call_encoder(ByNameEncoder::new("test-encoder-v1"));
    assert!(matches!(same.work_encoder(Some(&binding))?, Ok(Some(_))));
    // A malformed retained contract is an error, never a silent default.
    let mut malformed = record.clone();
    malformed.attempts[0].material["call_encoder"] = json!(7);
    for attempt in malformed.attempts.iter_mut().skip(1) {
        attempt
            .material
            .as_object_mut()
            .unwrap()
            .remove("call_encoder");
    }
    assert!(super::signed_encoder_contract(&malformed).is_err());
    assert!(signed.lock().unwrap().is_some());
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_encoder_composition_total 1"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_errors_total"),
        "{rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// The encoder contract is not part of an operation's identity: work queued
/// while an encoder was installed but composed from the compiled descriptor
/// restarts and submits without that encoder, and redelivering an operation
/// ID after installing an encoder stays idempotent.
#[tokio::test]
async fn encoder_installation_never_changes_retained_work() -> anyhow::Result<()> {
    let (lab, cx, account) = cr_lab_with("composer-restart", 4, |_| Ok(())).await?;
    let durable = |encoder: Option<Arc<ByNameEncoder>>| -> anyhow::Result<WeightScheduler> {
        let mut scheduler = scheduler()?
            .with_journal(
                lab.journal().unwrap().clone(),
                lab.client.genesis(),
                NetUid(1),
                account,
            )?
            .with_shield_protocol(lab.protocol.clone())
            .with_shielded_submission(ShieldPeriod::P4);
        if let Some(encoder) = encoder {
            scheduler = scheduler.with_call_encoder(encoder);
        }
        Ok(scheduler)
    };
    let input = WeightsInput::Parallel {
        uids: vec![Uid(1)],
        weights: vec![1.0],
    };
    let queued = durable(None)?;
    queued.enqueue("op-1", None, MechId(0), input.clone())?;
    drop(queued);
    let encoder = ByNameEncoder::new("test-encoder-v1");
    let redelivered = durable(Some(encoder.clone()))?;
    redelivered.enqueue("op-1", None, MechId(0), input.clone())?;
    drop(redelivered);
    let snapshot = lab.journal().unwrap().snapshot()?;
    assert_eq!(snapshot.operations.len(), 1);
    let effects = &snapshot.operations["op-1"].intent.effects;
    assert!(effects.get("call_encoder").is_none(), "{effects}");

    lab.after_inclusion(move |state, record, number| {
        let (ciphertext, round): (Vec<u8>, u64) =
            serde_json::from_value(record.material["commit"].clone()).expect("commit material");
        super::super::tests::seed_at(
            state,
            number,
            "SubtensorModule",
            "TimelockedWeightCommits",
            (NetUidStorageIndex(1), 5u64),
            vec![(account, number, ciphertext, round)],
        )
        .expect("native queue model");
    });
    // Restarted without any encoder, the retained work still submits.
    let mut restored = durable(None)?;
    until_authored(&mut restored, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    assert!(
        matches!(outcome(&restored), WeightAttemptOutcome::Submitted { .. }),
        "{:?}",
        outcome(&restored)
    );
    assert_eq!(encoder.calls.load(Ordering::SeqCst), 0);
    let record = lab.journal().unwrap().snapshot()?.operations["op-1"].clone();
    assert!(!record.attempts.is_empty());
    for attempt in &record.attempts {
        // Compiled material carries no contract and keeps its bytes.
        assert!(attempt.material.get("call_encoder").is_none());
        let material: super::Material = serde_json::from_value(attempt.material.clone())?;
        assert!(material.call_encoder.is_none());
        assert_eq!(serde_json::to_value(&material)?, attempt.material);
    }
    assert_eq!(super::signed_encoder_contract(&record)?, None);
    close_cr_lab(restored, lab, cx).await;
    Ok(())
}

/// Encodes `commit_crv3_mechanism_weights`, a `SubtensorModule` call outside
/// [`super::super::COMMIT_WEIGHT_CALLS`] that it declares.
struct DeclaredEncoder(AtomicUsize);
impl WeightCallEncoder for DeclaredEncoder {
    fn contract(&self) -> &str {
        "declared-v1"
    }
    fn encode(
        &self,
        runtime: &RuntimeHandle,
        call: &WeightCall<'_>,
    ) -> Result<Vec<u8>, Incompatibility> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let WeightCall::Commit {
            netuid,
            mechanism,
            commit,
            reveal_round,
            ..
        } = *call
        else {
            panic!("a commit-reveal lab prepares commits")
        };
        runtime
            .core()
            .compose_call(
                "SubtensorModule",
                "commit_crv3_mechanism_weights",
                &Value::record(vec![
                    ("netuid".into(), netuid.into_core_value()),
                    ("mecid".into(), mechanism.into_core_value()),
                    ("commit".into(), Value::Bytes(commit.to_vec())),
                    ("reveal_round".into(), reveal_round.into_core_value()),
                ]),
            )
            .map_err(|error| Incompatibility {
                code: "test-encoder".into(),
                descriptor: None,
                detail: error.to_string(),
                extensions: Default::default(),
            })
    }
    fn additional_calls(&self) -> &[&str] {
        &["commit_crv3_mechanism_weights"]
    }
}

fn assert_declared_refused(scheduler: &WeightScheduler) {
    let WeightAttemptOutcome::PausedSurface { detail, .. } = outcome(scheduler) else {
        panic!("expected a pause: {:?}", outcome(scheduler))
    };
    assert!(
        detail.contains("commit_crv3_mechanism_weights") && detail.contains("semantic guard"),
        "{detail}"
    );
}

/// An installed semantic guard alone does not admit an encoder's declared
/// calls: a non-durable scheduler never runs the guard, so the call refuses
/// unsigned.
#[tokio::test]
async fn declared_calls_refuse_where_no_guard_checks_the_attempt() -> anyhow::Result<()> {
    let (lab, cx, _) =
        cr_lab_with("composer-declared", 4, patched(extra_field(COMMIT_CALL))).await?;
    let encoder = Arc::new(DeclaredEncoder(AtomicUsize::new(0)));
    let guard = Arc::new(CountingGuard::default());
    let mut scheduler = scheduler()?
        .with_semantic_guard("test-semantics-v1", guard.clone())
        .with_call_encoder(encoder.clone());
    assert!(!scheduler.semantic_guard_runs(MechId(0))?);
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, scored)).await?;
        if encoder.0.load(Ordering::SeqCst) > 0 {
            break;
        }
    }
    assert!(encoder.0.load(Ordering::SeqCst) > 0, "the encoder ran");
    assert_eq!(lab.authors(), 0);
    assert_eq!(guard.0.load(Ordering::SeqCst), 0);
    assert_declared_refused(&scheduler);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// Durable work queued before a semantic contract existed is dispatched
/// without the guard, so a restart that installs one still refuses the
/// encoder's declared calls; work queued under the contract admits them.
#[tokio::test]
async fn declared_calls_refuse_for_work_queued_without_the_semantic_contract() -> anyhow::Result<()>
{
    let (lab, cx, account) = cr_lab_with(
        "composer-declared-durable",
        4,
        patched(extra_field(COMMIT_CALL)),
    )
    .await?;
    let durable = || -> anyhow::Result<WeightScheduler> {
        Ok(scheduler()?
            .with_journal(
                lab.journal().unwrap().clone(),
                lab.client.genesis(),
                NetUid(1),
                account,
            )?
            .with_shield_protocol(lab.protocol.clone())
            .with_shielded_submission(ShieldPeriod::P4))
    };
    let input = WeightsInput::Parallel {
        uids: vec![Uid(1)],
        weights: vec![1.0],
    };
    let queued = durable()?;
    queued.enqueue("unguarded", None, MechId(0), input.clone())?;
    drop(queued);
    let encoder = Arc::new(DeclaredEncoder(AtomicUsize::new(0)));
    let guard = Arc::new(CountingGuard::default());
    let mut restarted = durable()?
        .with_semantic_guard("test-semantics-v1", guard.clone())
        .with_call_encoder(encoder.clone());
    assert!(!restarted.semantic_guard_runs(MechId(0))?);
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(restarted.on_finalized(&cx, &head, scored)).await?;
        if encoder.0.load(Ordering::SeqCst) > 0 {
            break;
        }
    }
    assert!(encoder.0.load(Ordering::SeqCst) > 0, "the encoder ran");
    assert_eq!(lab.authors(), 0);
    assert_eq!(guard.0.load(Ordering::SeqCst), 0);
    assert_declared_refused(&restarted);
    let record = lab.journal().unwrap().snapshot()?.operations["unguarded"].clone();
    assert!(record.attempts.is_empty());

    // Work queued under the installed contract runs the guard on dispatch,
    // which is what admits a declared call.
    restarted.persist(Some("unguarded"))?;
    restarted.enqueue("guarded", None, MechId(0), input)?;
    assert!(restarted.semantic_guard_runs(MechId(0))?);
    close_cr_lab(restarted, lab, cx).await;
    Ok(())
}

/// A fee-free `IncorrectCommitRevealVersion` through an encoder still
/// re-selects at most [`super::super::CR_VERSION_RETRIES`] times (D-086).
#[tokio::test]
async fn encoder_commits_keep_the_bounded_tag_retry() -> anyhow::Result<()> {
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let rejection = json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":cr_pallet,"error":[cr_error,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"No"}},"topics":[]});
    let (lab, cx, account) =
        cr_lab_with("composer-retry", 4, patched(extra_field(COMMIT_CALL))).await?;
    let encoder = ByNameEncoder::new("test-encoder-v1");
    let mut scheduler = scheduler()?.with_call_encoder(encoder.clone());
    let tails = plain_author(&lab, account, move |state, _| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: json!([rejection.clone()]),
        });
    });
    until_authored(&mut scheduler, &lab, &cx).await?;
    let retries = usize::try_from(super::super::CR_VERSION_RETRIES)?;
    assert_eq!(lab.authors(), 1 + retries);
    assert_eq!(encoder.calls.load(Ordering::SeqCst), 1 + retries);
    assert!(tails.lock().unwrap().iter().all(|tail| tail.2 == 4));
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains(&format!(
            "rtao_weights_cr_version_recomputed_total {retries}"
        )),
        "{rendered}"
    );
    assert!(matches!(
        outcome(&scheduler),
        WeightAttemptOutcome::Failed(_)
    ));
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// Executor policy still applies to bytes composed outside the compiled
/// descriptors: a subnet outside `allowed_netuids` is refused unsigned.
#[tokio::test]
async fn verified_bytes_still_meet_the_executor_policy() -> anyhow::Result<()> {
    let (lab, mut cx, _) = cr_lab_with("composer-policy", 4, patched(unbounded_commit)).await?;
    cx.executor = rtao_tx::Executor::new(lab.client.clone()).with_policy(rtao_tx::Policy {
        allowed_netuids: Some(std::collections::BTreeSet::from([NetUid(2)])),
        ..Default::default()
    });
    let mut scheduler = scheduler()?;
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, scored)).await?;
        if matches!(outcome(&scheduler), WeightAttemptOutcome::Failed(_)) {
            break;
        }
    }
    assert_eq!(lab.authors(), 0);
    let WeightAttemptOutcome::Failed(error) = outcome(&scheduler) else {
        panic!("expected a policy refusal: {:?}", outcome(&scheduler))
    };
    assert!(error.contains("netuid"), "{error}");
    assert!(
        cx.metrics
            .render()
            .contains("rtao_weights_resolved_composition_total 1")
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

mod encoder_validation {
    use super::super::super::compose::{self, Composer};
    use super::*;
    use rtao_core::runtime::Runtime;

    fn pinned() -> Runtime {
        Runtime::parse(
            include_bytes!("../../../../vendor/metadata/finney/488.v15.scale"),
            488,
            1,
            42,
        )
        .expect("pinned metadata")
    }

    fn runtime() -> RuntimeHandle {
        RuntimeHandle::new(pinned()).expect("runtime")
    }

    const DESTS: [u16; 2] = [0, 3];
    const WEIGHTS: [u16; 2] = [u16::MAX, 1];

    fn set_call(mechanism: u8) -> WeightCall<'static> {
        WeightCall::Set {
            netuid: NetUid(1),
            mechanism: MechId(mechanism),
            dests: &DESTS,
            weights: &WEIGHTS,
            version_key: 9,
        }
    }

    fn commit_call() -> WeightCall<'static> {
        WeightCall::Commit {
            netuid: NetUid(1),
            mechanism: MechId(0),
            commit: &[1, 2, 3],
            reveal_round: 77,
            commit_reveal_version: 4,
        }
    }

    /// Returns fixed bytes, optionally declaring further call names.
    struct Fixed(Vec<u8>, &'static [&'static str]);
    impl WeightCallEncoder for Fixed {
        fn contract(&self) -> &str {
            "fixed"
        }
        fn encode(
            &self,
            _: &RuntimeHandle,
            _: &WeightCall<'_>,
        ) -> Result<Vec<u8>, Incompatibility> {
            Ok(self.0.clone())
        }
        fn additional_calls(&self) -> &[&str] {
            self.1
        }
    }

    fn compiled_set(runtime: &RuntimeHandle, netuid: u16, mechanism: u8) -> Vec<u8> {
        rtao_runtime::calls::subtensor_module::set_mechanism_weights(
            NetUid(netuid),
            MechId(mechanism),
            DESTS.to_vec(),
            WEIGHTS.to_vec(),
            9,
        )
        .compose(runtime)
        .unwrap()
        .as_bytes()
        .to_vec()
    }

    fn refusal(
        runtime: &RuntimeHandle,
        bytes: Vec<u8>,
        call: &WeightCall<'_>,
        guarded: bool,
    ) -> Incompatibility {
        compose::encode_application(&Fixed(bytes, &[]), runtime, call, guarded)
            .expect_err("validation must refuse")
    }

    #[test]
    fn faithful_bytes_import_and_match_the_compiled_call() {
        let runtime = runtime();
        let bytes = compiled_set(&runtime, 1, 0);
        let imported =
            compose::encode_application(&Fixed(bytes.clone(), &[]), &runtime, &set_call(0), false)
                .unwrap();
        assert_eq!(imported.as_bytes(), bytes);
        let encoder = ByNameEncoder::new("by-name");
        let commit =
            compose::encode_application(encoder.as_ref(), &runtime, &commit_call(), false).unwrap();
        let compiled = rtao_runtime::calls::subtensor_module::commit_timelocked_mechanism_weights(
            NetUid(1),
            MechId(0),
            vec![1, 2, 3],
            77,
            4,
        )
        .compose(&runtime)
        .unwrap();
        assert_eq!(commit, compiled);
        // Legacy `set_weights` has no mechanism field: only mechanism zero.
        let legacy = runtime
            .core()
            .compose_call(
                "SubtensorModule",
                "set_weights",
                &Value::record(vec![
                    ("netuid".into(), NetUid(1).into_core_value()),
                    ("dests".into(), DESTS.to_vec().into_core_value()),
                    ("weights".into(), WEIGHTS.to_vec().into_core_value()),
                    ("version_key".into(), 9u64.into_core_value()),
                ]),
            )
            .unwrap();
        compose::encode_application(&Fixed(legacy.clone(), &[]), &runtime, &set_call(0), false)
            .unwrap();
        let missing = refusal(&runtime, legacy, &set_call(2), false);
        assert_eq!(missing.code, "weight-encoder-value");
        assert!(missing.detail.contains("mecid"), "{}", missing.detail);
    }

    #[test]
    fn mismatched_values_and_calls_refuse() {
        let runtime = runtime();
        let wrong_netuid = refusal(&runtime, compiled_set(&runtime, 2, 0), &set_call(0), false);
        assert_eq!(wrong_netuid.code, "weight-encoder-value");
        assert!(wrong_netuid.detail.contains("netuid"));
        let wrong_mechanism = refusal(&runtime, compiled_set(&runtime, 1, 1), &set_call(0), false);
        assert!(wrong_mechanism.detail.contains("mecid"));
        let other_row = rtao_runtime::calls::subtensor_module::set_mechanism_weights(
            NetUid(1),
            MechId(0),
            vec![0, 4],
            WEIGHTS.to_vec(),
            9,
        )
        .compose(&runtime)
        .unwrap();
        let wrong_row = refusal(&runtime, other_row.as_bytes().to_vec(), &set_call(0), false);
        assert!(wrong_row.detail.contains("dests"), "{}", wrong_row.detail);
        // A direct row where a commitment was prepared.
        let wrong_kind = refusal(
            &runtime,
            compiled_set(&runtime, 1, 0),
            &commit_call(),
            false,
        );
        assert_eq!(wrong_kind.code, "weight-encoder-call");
        let remark = rtao_runtime::calls::system::remark(vec![1])
            .compose(&runtime)
            .unwrap();
        let other = refusal(&runtime, remark.as_bytes().to_vec(), &set_call(0), false);
        assert_eq!(other.code, "weight-encoder-call");
        let inner =
            rtao_runtime::CallBytes::decode(&compiled_set(&runtime, 1, 0), &runtime).unwrap();
        let batch = rtao_runtime::calls::utility::batch(vec![inner])
            .compose(&runtime)
            .unwrap();
        let wrapped = refusal(&runtime, batch.as_bytes().to_vec(), &set_call(0), false);
        assert_eq!(wrapped.code, "weight-encoder-call");
    }

    #[test]
    fn non_canonical_and_trailing_bytes_refuse() {
        let runtime = runtime();
        let mut trailing = compiled_set(&runtime, 1, 0);
        trailing.push(0);
        assert_eq!(
            refusal(&runtime, trailing, &set_call(0), false).code,
            "weight-encoder-bytes"
        );
        // `dests` length 2 in the two-byte compact mode instead of one byte.
        let mut wide = compiled_set(&runtime, 1, 0);
        assert_eq!(wide[5], 2 << 2);
        wide.splice(5..6, [(2 << 2) | 1, 0]);
        assert_eq!(
            refusal(&runtime, wide, &set_call(0), false).code,
            "weight-encoder-bytes"
        );
        assert_eq!(
            refusal(&runtime, vec![], &set_call(0), false).code,
            "weight-encoder-bytes"
        );
    }

    #[test]
    fn declared_calls_need_a_semantic_guard_and_nested_calls_refuse() {
        let runtime = runtime();
        let bytes = compiled_set(&runtime, 1, 0);
        // Pretend the direct-row call is unknown: declared names need a guard.
        let declared = Fixed(bytes.clone(), &["set_mechanism_weights"]);
        compose::encode_application(&declared, &runtime, &commit_call(), true)
            .expect_err("a direct row never satisfies a commitment");
        let remark_declared = Fixed(
            rtao_runtime::calls::system::remark(vec![1])
                .compose(&runtime)
                .unwrap()
                .as_bytes()
                .to_vec(),
            &["remark"],
        );
        assert_eq!(
            compose::encode_application(&remark_declared, &runtime, &set_call(0), true)
                .unwrap_err()
                .code,
            "weight-encoder-call",
            "only SubtensorModule names can be declared"
        );

        // A runtime whose direct-row call gained a trailing nested call.
        let mut core = pinned();
        let call_type = core.extrinsic.call_type.unwrap();
        let calls_type = core.pallet("SubtensorModule").unwrap().calls_type.unwrap();
        let ty = core
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == calls_type)
            .unwrap();
        let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
            panic!("call enum")
        };
        variants
            .variants
            .iter_mut()
            .find(|variant| variant.name == SET_CALL)
            .unwrap()
            .fields
            .push(Field {
                name: Some("then".into()),
                ty: call_type.into(),
                type_name: None,
                docs: vec![],
            });
        let nested = RuntimeHandle::new(core).unwrap();
        let remark = rtao_runtime::calls::system::remark(vec![1])
            .compose(&nested)
            .unwrap();
        let bytes = [compiled_set(&runtime, 1, 0), remark.as_bytes().to_vec()].concat();
        let refused = compose::encode_application(&Fixed(bytes, &[]), &nested, &set_call(0), true)
            .unwrap_err();
        assert_eq!(refused.code, "weight-encoder-nested", "{}", refused.detail);
        // Name resolution refuses the same call, so it would pause.
        assert!(matches!(
            compose::select(&nested, false, None),
            Err(detail) if detail.contains("then")
        ));
        let encoder: Arc<dyn WeightCallEncoder> = ByNameEncoder::new("by-name");
        assert!(matches!(
            compose::select(&nested, false, Some(&encoder)),
            Ok(Composer::Application(_))
        ));
        assert!(matches!(
            compose::select(&runtime, false, Some(&encoder)),
            Ok(Composer::Compiled)
        ));
    }

    fn spec(version: u32) -> RuntimeHandle {
        let mut core = pinned();
        core.spec_version = version;
        RuntimeHandle::new(core).expect("runtime")
    }

    /// Verified bytes are composed under the best-head preflight runtime but
    /// signed under the executor's refreshed runtime. Across an upgrade
    /// between the two, the same composer composes again for the signing
    /// runtime and validates again, instead of failing the attempt after the
    /// cadence slot was reserved.
    #[test]
    fn verified_calls_compose_again_for_a_different_signing_runtime() {
        use super::super::super::submit::{VerifiedCall, WeightCallPlan};
        let preflight = runtime();
        let upgraded = spec(487);
        let plan = WeightCallPlan {
            netuid: NetUid(1),
            mechanism: MechId(0),
            row: (vec![], vec![]),
            version_key: 0,
            commit: Some(rtao_intents::weights::PreparedWeightCommit {
                ciphertext: vec![1, 2, 3],
                reveal_round: 77,
            }),
            commit_reveal_version: 4,
        };
        let encoder = ByNameEncoder::new("by-name");
        for composer in [Composer::Resolved, Composer::Application(encoder.clone())] {
            let composed = plan
                .with_call(|call| compose::compose_verified(&composer, &preflight, call, false))
                .expect("not compiled")
                .unwrap();
            let verified = VerifiedCall::new(composed.clone(), composer.clone(), false);
            let before = encoder.calls.load(Ordering::SeqCst);
            assert_eq!(verified.for_runtime(&plan, &preflight).unwrap(), composed);
            assert_eq!(encoder.calls.load(Ordering::SeqCst), before);
            let signing = verified.for_runtime(&plan, &upgraded).unwrap();
            assert_eq!(signing.stamp(), upgraded.stamp());
            assert_eq!(signing.as_bytes(), composed.as_bytes());
            signing.ensure_runtime(&upgraded).unwrap();
        }
        // The signing runtime's own validation decides: an added field that
        // name resolution refuses fails the build unsigned, and the encoder
        // that supplies it composes.
        let mut core = pinned();
        core.spec_version = 489;
        let calls_type = core.pallet("SubtensorModule").unwrap().calls_type.unwrap();
        let u64 = core
            .types
            .types
            .iter()
            .find(|ty| matches!(&ty.ty.type_def, TypeDef::Primitive(TypeDefPrimitive::U64)))
            .unwrap()
            .id;
        let ty = core
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == calls_type)
            .unwrap();
        let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
            panic!("call enum")
        };
        variants
            .variants
            .iter_mut()
            .find(|variant| variant.name == COMMIT_CALL)
            .unwrap()
            .fields
            .insert(
                0,
                Field {
                    name: Some("extra".into()),
                    ty: u64.into(),
                    type_name: Some("u64".into()),
                    docs: vec![],
                },
            );
        let changed = RuntimeHandle::new(core).unwrap();
        let composed = plan
            .with_call(|call| compose::compose_resolved(&preflight, call))
            .unwrap();
        let refused = VerifiedCall::new(composed.clone(), Composer::Resolved, false)
            .for_runtime(&plan, &changed)
            .unwrap_err();
        assert!(
            refused.to_string().contains("signing runtime (spec 489)")
                && refused.to_string().contains("extra"),
            "{refused}"
        );
        let encoded = VerifiedCall::new(composed, Composer::Application(encoder), false)
            .for_runtime(&plan, &changed)
            .unwrap();
        assert_eq!(encoded.stamp(), changed.stamp());
    }

    #[test]
    fn encoder_errors_pass_through() {
        struct Refusing;
        impl WeightCallEncoder for Refusing {
            fn contract(&self) -> &str {
                "refusing"
            }
            fn encode(
                &self,
                _: &RuntimeHandle,
                _: &WeightCall<'_>,
            ) -> Result<Vec<u8>, Incompatibility> {
                Err(Incompatibility {
                    code: "app-shape".into(),
                    descriptor: Some("call:SubtensorModule.set_mechanism_weights".into()),
                    detail: "unknown runtime".into(),
                    extensions: Default::default(),
                })
            }
        }
        let refused =
            compose::encode_application(&Refusing, &runtime(), &set_call(0), false).unwrap_err();
        assert_eq!(refused.code, "app-shape");
    }
}
