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
/// encoder contract with the durable work.
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
    assert_eq!(
        record.intent.effects["call_encoder"],
        json!("test-encoder-v1")
    );
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

/// Retained work queued under an encoder contract is rebuilt only while an
/// encoder with that contract is installed; native work keeps its bytes.
#[tokio::test]
async fn retained_encoder_work_waits_for_its_encoder_after_restart() -> anyhow::Result<()> {
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
    let queued = durable(Some(ByNameEncoder::new("test-encoder-v1")))?;
    queued.enqueue(
        "op-1",
        None,
        MechId(0),
        WeightsInput::Parallel {
            uids: vec![Uid(1)],
            weights: vec![1.0],
        },
    )?;
    drop(queued);
    for encoder in [None, Some(ByNameEncoder::new("test-encoder-v2"))] {
        let mut restarted = durable(encoder)?;
        let head = advance(&lab)?;
        Box::pin(restarted.on_finalized(&cx, &head, scored)).await?;
        assert_eq!(lab.authors(), 0);
        let WeightAttemptOutcome::Unresolved(reason) = outcome(&restarted) else {
            panic!("expected unresolved work: {:?}", outcome(&restarted))
        };
        assert!(
            reason.contains("call encoder test-encoder-v1 required"),
            "{reason}"
        );
    }
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
    let encoder = ByNameEncoder::new("test-encoder-v1");
    let mut restored = durable(Some(encoder.clone()))?;
    until_authored(&mut restored, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    // The compiled descriptor matches, so the encoder is installed but unused.
    assert_eq!(encoder.calls.load(Ordering::SeqCst), 0);
    close_cr_lab(restored, lab, cx).await;

    let native = super::Work {
        netuid: NetUid(1),
        mechanism: MechId(0),
        input: WeightsInput::RawU16 {
            uids: vec![Uid(1)],
            values: vec![1],
        },
        version_key: 0,
        call_encoder: None,
    };
    let encoded = serde_json::to_value(&native)?;
    assert!(encoded.get("call_encoder").is_none(), "{encoded}");
    let _: super::Work = serde_json::from_value(encoded)?;
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
            include_bytes!("../../../../vendor/metadata/finney/486.v15.scale"),
            486,
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
