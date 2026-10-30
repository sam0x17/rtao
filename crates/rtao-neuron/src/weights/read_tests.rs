//! Validator storage reads after a runtime change (D-089): the name-resolved
//! route decodes the same values as the compiled one, benign shape changes
//! keep the run path reading, and a change whose meaning is lost refuses only
//! the reads of that entry.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::tests::{HOTKEY, fixture, fixture_with};
use super::*;
use crate::reads;
use frame_metadata::v15::{RuntimeMetadataV15, StorageEntryModifier, StorageEntryType};
use rtao_client::ViewExt;
use scale_info::{
    Field, Path, PortableType, Type, TypeDef, TypeDefComposite, TypeDefPrimitive,
    form::PortableForm,
};

/// Append a type to served metadata.
pub(super) fn add_type(
    meta: &mut RuntimeMetadataV15,
    path: &[&str],
    type_def: TypeDef<PortableForm>,
) -> u32 {
    let id = u32::try_from(meta.types.types.len()).expect("type id");
    meta.types.types.push(PortableType {
        id,
        ty: Type {
            path: Path {
                segments: path.iter().map(|segment| (*segment).to_owned()).collect(),
            },
            type_params: vec![],
            type_def,
            docs: vec![],
        },
    });
    id
}

pub(super) fn field(name: Option<&str>, ty: u32) -> Field<PortableForm> {
    Field {
        name: name.map(Into::into),
        ty: ty.into(),
        type_name: None,
        docs: vec![],
    }
}

fn entry<'a>(
    meta: &'a mut RuntimeMetadataV15,
    pallet: &str,
    name: &str,
) -> &'a mut frame_metadata::v15::StorageEntryMetadata<PortableForm> {
    meta.pallets
        .iter_mut()
        .find(|candidate| candidate.name == pallet)
        .and_then(|pallet| pallet.storage.as_mut())
        .and_then(|storage| storage.entries.iter_mut().find(|entry| entry.name == name))
        .expect("storage entry")
}

fn value_type(meta: &mut RuntimeMetadataV15, name: &str) -> u32 {
    match &entry(meta, "SubtensorModule", name).ty {
        StorageEntryType::Plain(ty) | StorageEntryType::Map { value: ty, .. } => ty.id,
    }
}

/// Point a SubtensorModule entry's value at `ty` with a new default.
pub(super) fn retype(meta: &mut RuntimeMetadataV15, name: &str, ty: u32, default: Vec<u8>) {
    let entry = entry(meta, "SubtensorModule", name);
    match &mut entry.ty {
        StorageEntryType::Plain(value) | StorageEntryType::Map { value, .. } => *value = ty.into(),
    }
    entry.default = default;
}

/// A u32 SubtensorModule value where the build reads u16.
pub(super) fn widen(meta: &mut RuntimeMetadataV15, name: &str, primitive: TypeDefPrimitive) {
    let default_len = match primitive {
        TypeDefPrimitive::U32 => 4,
        TypeDefPrimitive::U128 => 16,
        _ => panic!("unsupported widening"),
    };
    let mut default = entry(meta, "SubtensorModule", name).default.clone();
    default.resize(default_len, 0);
    let ty = add_type(meta, &[], TypeDef::Primitive(primitive));
    retype(meta, name, ty, default);
}

/// Wrap a SubtensorModule value in an unnamed single-field newtype.
fn bound(meta: &mut RuntimeMetadataV15, name: &str) {
    let inner = value_type(meta, name);
    let default = entry(meta, "SubtensorModule", name).default.clone();
    let ty = add_type(
        meta,
        &["bounded_collections", "BoundedVec"],
        TypeDef::Composite(TypeDefComposite {
            fields: vec![field(None, inner)],
        }),
    );
    retype(meta, name, ty, default);
}

/// The `TimelockedWeightCommits` element tuple's field types.
pub(super) fn commit_fields(meta: &mut RuntimeMetadataV15) -> [u32; 4] {
    let queue = value_type(meta, "TimelockedWeightCommits");
    let element = match &meta.types.types[queue as usize].ty.type_def {
        TypeDef::Sequence(sequence) => sequence.type_param.id,
        other => panic!("commit queue is {other:?}"),
    };
    match &meta.types.types[element as usize].ty.type_def {
        TypeDef::Tuple(tuple) => {
            <[u32; 4]>::try_from(tuple.fields.iter().map(|f| f.id).collect::<Vec<_>>())
                .expect("four fields")
        }
        other => panic!("commit is {other:?}"),
    }
}

/// Replace the commit queue element with `element`.
pub(super) fn commit_element(meta: &mut RuntimeMetadataV15, element: u32) {
    let queue = add_type(
        meta,
        &[],
        TypeDef::Sequence(scale_info::TypeDefSequence {
            type_param: element.into(),
        }),
    );
    retype(meta, "TimelockedWeightCommits", queue, vec![0]);
}

/// The commit tuple as a named struct with reordered fields, an alias for
/// the round and an added field the validator does not read.
pub(super) fn reordered_commits(meta: &mut RuntimeMetadataV15) {
    let [who, block, commit, round] = commit_fields(meta);
    let extra = add_type(meta, &[], TypeDef::Primitive(TypeDefPrimitive::U32));
    let element = add_type(
        meta,
        &["pallet_subtensor", "TimelockedCommit"],
        TypeDef::Composite(TypeDefComposite {
            fields: vec![
                field(Some("commit_block"), block),
                field(Some("hotkey"), who),
                field(Some("round_number"), round),
                field(Some("weight_count"), extra),
                field(Some("commit"), commit),
            ],
        }),
    );
    commit_element(meta, element);
}

const UNREGISTERED: AccountId = AccountId([9; 32]);

fn by_name_error(error: &anyhow::Error) -> &rtao_runtime::ReadResolveError {
    match error.downcast_ref::<rtao_client::ClientError>() {
        Some(rtao_client::ClientError::ReadResolveIncompatible(error)) => error,
        _ => panic!("expected a name-resolved read refusal, got {error:#}"),
    }
}

#[test]
fn the_name_resolved_table_covers_every_validator_storage_read_in_order() {
    let compiled = PREFLIGHT_STORAGE
        .iter()
        .chain(&RUN_PATH_STORAGE)
        .map(|surface| (surface.pallet, surface.name, surface.key_type_idents.len()))
        .collect::<Vec<_>>();
    let resolved = reads::VALIDATOR_READS
        .iter()
        .map(|item| (item.pallet, item.name, item.keys.len()))
        .collect::<Vec<_>>();
    assert_eq!(compiled, resolved);
    let uids = &reads::VALIDATOR_READS[0];
    assert_eq!(uids.presence, rtao_runtime::ReadPresence::Optional);
    assert!(
        reads::VALIDATOR_READS[1..]
            .iter()
            .all(|item| item.presence == rtao_runtime::ReadPresence::Default)
    );
}

/// On the pinned runtime the name-resolved route decodes every validator
/// read to the values the compiled route returns.
#[tokio::test]
async fn by_name_reads_equal_compiled_reads_on_the_pinned_runtime() -> anyhow::Result<()> {
    let (chain, client) = fixture().await?;
    chain.mutate_state(|state| -> anyhow::Result<()> {
        state.seed(&st::MECHANISM_COUNT_CURRENT, (NetUid(1),), MechId(2))?;
        state.seed(&st::ACTIVITY_CUTOFF_FACTOR_MILLI, (NetUid(1),), 1_500u32)?;
        state.seed(&st::STAKE_THRESHOLD, (), 5u64)?;
        state.seed_typed("SubtensorModule", "SubnetOwnerHotkey", (NetUid(1),), HOTKEY)?;
        state.seed(
            &st::TIMELOCKED_WEIGHT_COMMITS,
            (NetUidStorageIndex(1), 6u64),
            vec![
                (HOTKEY, 110u64, vec![4u8, 5, 6], 500u64),
                (AccountId([8; 32]), 111u64, vec![], 501u64),
            ],
        )?;
        state.seed(&rtao_runtime::storage::drand::LAST_STORED_ROUND, (), 499u64)?;
        // An unregistered hotkey's UID is a miss, not the fake's default.
        let uids = state
            .runtime
            .storage_entry("SubtensorModule", "Uids")
            .expect("Uids");
        let key = state
            .runtime
            .storage_key(
                uids,
                &[NetUid(1).into_core_value(), UNREGISTERED.into_core_value()],
            )
            .map_err(rtao_core::CoreError::from)?;
        state.seed_encoded(key, None);
        Ok(())
    })?;
    let best = client.at(BlockRef::Best).await?;
    let runtime = best.block().runtime.clone();
    for surface in PREFLIGHT_STORAGE.iter().chain(&RUN_PATH_STORAGE) {
        assert_eq!(reads::route(&runtime, surface)?, reads::Route::Compiled);
    }
    for hotkey in [HOTKEY, UNREGISTERED] {
        let plan = WeightPreflight::reads(NetUid(1), MechId(0), hotkey)?;
        let compiled = reads::fetch_dyn(&best, plan.clone()).await?;
        assert_eq!(compiled, reads::fetch_dyn_by_name(&best, plan).await?);
    }
    let auxiliary = || {
        (
            st::MECHANISM_COUNT_CURRENT.at((NetUid(1),)),
            st::ACTIVITY_CUTOFF_FACTOR_MILLI.at((NetUid(1),)),
            st::STAKE_THRESHOLD.at(()),
            st::SUBNET_OWNER_HOTKEY.at((NetUid(1),)),
        )
    };
    let compiled = reads::fetch(&best, auxiliary()).await?;
    assert_eq!(compiled.0, MechId(2));
    assert_eq!(compiled, reads::fetch_by_name(&best, auxiliary()).await?);
    let first_check = |hotkey: AccountId| {
        (
            st::UIDS.at((NetUid(1), hotkey)),
            st::VALIDATOR_PERMIT.at((NetUid(1),)),
            st::STAKE_THRESHOLD.at(()),
            st::WEIGHTS_VERSION_KEY.at((NetUid(1),)),
            st::SUBNETWORK_N.at((NetUid(1),)),
            st::MIN_ALLOWED_WEIGHTS.at((NetUid(1),)),
            st::SUBNET_OWNER_HOTKEY.at((NetUid(1),)),
            st::SUBNET_EPOCH_CONSENSUS.at((NetUid(1),)),
        )
    };
    for hotkey in [HOTKEY, UNREGISTERED] {
        let compiled = reads::fetch(&best, first_check(hotkey)).await?;
        assert_eq!(compiled.0.is_some(), hotkey == HOTKEY);
        assert_eq!(
            compiled,
            reads::fetch_by_name(&best, first_check(hotkey)).await?
        );
    }
    let schedule = || {
        (
            st::TEMPO.at((NetUid(1),)),
            st::LAST_EPOCH_BLOCK.at((NetUid(1),)),
            st::PENDING_EPOCH_AT.at((NetUid(1),)),
            st::SUBNET_EPOCH_INDEX.at((NetUid(1),)),
            st::BLOCKS_SINCE_LAST_STEP.at((NetUid(1),)),
        )
    };
    assert_eq!(
        reads::fetch(&best, schedule()).await?,
        reads::fetch_by_name(&best, schedule()).await?
    );
    for epoch in [6u64, 7] {
        let queue = || st::TIMELOCKED_WEIGHT_COMMITS.at((NetUidStorageIndex(1), epoch));
        let compiled = reads::fetch(&best, queue()).await?;
        assert_eq!(compiled.len(), if epoch == 6 { 2 } else { 0 });
        assert_eq!(compiled, reads::fetch_by_name(&best, queue()).await?);
    }
    let round = || rtao_runtime::storage::drand::LAST_STORED_ROUND.at(());
    assert_eq!(reads::fetch(&best, round()).await?, 499);
    assert_eq!(reads::fetch_by_name(&best, round()).await?, 499);
    let version = || st::COMMIT_REVEAL_WEIGHTS_VERSION.at(());
    assert_eq!(
        reads::fetch(&best, version()).await?,
        reads::fetch_by_name(&best, version()).await?
    );
    let compiled = best
        .constant(&rtao_runtime::constants::aura::SLOT_DURATION)
        .await?;
    let resolved =
        rtao_runtime::read_constant(&runtime, "Aura", "SlotDuration", &reads::SLOT_DURATION)?;
    assert_eq!(resolved.as_uint(), Some(u128::from(compiled)));
    client.close().await;
    Ok(())
}

/// A widened integer and a `BoundedVec` keep the preflight reading the same
/// values in one coalesced RPC.
#[tokio::test]
async fn preflight_reads_widened_and_bounded_entries_by_name_in_one_rpc() -> anyhow::Result<()> {
    let (_, pinned) = fixture().await?;
    let expected = WeightPreflight::fetch(
        &pinned.at(BlockRef::Best).await?,
        NetUid(1),
        MechId(0),
        HOTKEY,
    )
    .await?;
    pinned.close().await;
    let (chain, client) = fixture_with(|meta| {
        widen(meta, "Tempo", TypeDefPrimitive::U32);
        bound(meta, "LastUpdate");
    })
    .await?;
    let best = client.at(BlockRef::Best).await?;
    let runtime = best.block().runtime.clone();
    for name in ["Tempo", "LastUpdate"] {
        let id = format!("storage:SubtensorModule.{name}");
        assert!(!runtime.surface_report().shape_matches(&id), "{id}");
    }
    assert_eq!(
        reads::route(&runtime, &st::TEMPO.surface())?,
        reads::Route::Resolved
    );
    let before = chain.requests().len();
    let state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    let storage = chain.requests()[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .map(|request| request.params[0].as_array().expect("keys").len())
        .collect::<Vec<_>>();
    assert_eq!(storage, [17]);
    assert_eq!(state.uid, expected.uid);
    assert_eq!(state.last_updates, expected.last_updates);
    assert_eq!(state.last_updates, [80, 0, 0]);
    assert_eq!(state.rate_limit, expected.rate_limit);
    assert_eq!(state.commit_reveal, expected.commit_reveal);
    assert_eq!(state.commit_reveal_version, expected.commit_reveal_version);
    assert_eq!(state.reveal_period, expected.reveal_period);
    assert_eq!(state.required_version_key, expected.required_version_key);
    assert_eq!(state.min_allowed_weights, expected.min_allowed_weights);
    assert_eq!(state.max_weights_limit, expected.max_weights_limit);
    assert_eq!(state.population, expected.population);
    assert_eq!(state.null_consensus, expected.null_consensus);
    assert_eq!(state.permits, expected.permits);
    assert_eq!(state.schedule, expected.schedule);
    assert_eq!(state.schedule.tempo, 10);
    // The schedule read by the reveal watcher resolves the same way.
    let (tempo, ..) = reads::fetch(
        &best,
        (
            st::TEMPO.at((NetUid(1),)),
            st::LAST_EPOCH_BLOCK.at((NetUid(1),)),
        ),
    )
    .await?;
    assert_eq!(tempo, 10);
    // A value beyond what the build's u16 holds refuses that read only.
    chain.mutate_state(|state| {
        state.seed_typed("SubtensorModule", "Tempo", (NetUid(1),), 70_000u32)
    })?;
    let best = client.at(BlockRef::Best).await?;
    let error = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY)
        .await
        .expect_err("out of range");
    let refusal = by_name_error(&error);
    assert_eq!(refusal.item, "Tempo");
    assert!(refusal.detail.contains("70000"), "{refusal}");
    client.close().await;
    Ok(())
}

/// A refused entry stops only the reads that need it.
#[tokio::test]
async fn a_refused_entry_fails_alone_with_its_name() -> anyhow::Result<()> {
    let (_, client) = fixture_with(|meta| {
        let signed = add_type(meta, &[], TypeDef::Primitive(TypeDefPrimitive::I64));
        retype(meta, "ActivityCutoffFactorMilli", signed, vec![0; 8]);
    })
    .await?;
    let best = client.at(BlockRef::Best).await?;
    let error = reads::fetch(
        &best,
        (
            st::MECHANISM_COUNT_CURRENT.at((NetUid(1),)),
            st::ACTIVITY_CUTOFF_FACTOR_MILLI.at((NetUid(1),)),
            st::STAKE_THRESHOLD.at(()),
            st::SUBNET_OWNER_HOTKEY.at((NetUid(1),)),
        ),
    )
    .await
    .expect_err("a signed cutoff has no recoverable meaning");
    let refusal = by_name_error(&error);
    assert_eq!(refusal.item, "ActivityCutoffFactorMilli");
    assert_eq!(refusal.part, "value");
    assert!(refusal.detail.contains("i64"), "{refusal}");
    // The preflight and the reveal schedule do not read it.
    WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    reads::storage(&best, &st::SUBNET_EPOCH_INDEX, (NetUid(1),)).await?;
    client.close().await;
    Ok(())
}

/// Durable recovery reads the inclusion epoch index after it is widened.
#[tokio::test]
async fn durable_epoch_index_reads_by_name_after_widening() -> anyhow::Result<()> {
    let (chain, client) =
        fixture_with(|meta| widen(meta, "SubnetEpochIndex", TypeDefPrimitive::U128)).await?;
    let best = client.at(BlockRef::Best).await?;
    assert_eq!(
        reads::storage(&best, &st::SUBNET_EPOCH_INDEX, (NetUid(1),)).await?,
        5
    );
    chain.mutate_state(|state| {
        state.seed_typed(
            "SubtensorModule",
            "SubnetEpochIndex",
            (NetUid(1),),
            u128::from(u64::MAX) + 1,
        )
    })?;
    let best = client.at(BlockRef::Best).await?;
    let error = reads::storage(&best, &st::SUBNET_EPOCH_INDEX, (NetUid(1),))
        .await
        .expect_err("beyond u64");
    assert_eq!(by_name_error(&error).item, "SubnetEpochIndex");
    client.close().await;
    Ok(())
}

/// An entry whose absence means "unregistered" refuses once a miss would
/// read as a default UID, on the compiled route too.
#[tokio::test]
async fn a_uid_entry_that_lost_its_optional_modifier_refuses() -> anyhow::Result<()> {
    let (_, client) = fixture_with(|meta| {
        entry(meta, "SubtensorModule", "Uids").modifier = StorageEntryModifier::Default;
    })
    .await?;
    let best = client.at(BlockRef::Best).await?;
    assert!(
        best.block()
            .runtime
            .surface_report()
            .shape_matches("storage:SubtensorModule.Uids"),
        "the exact shape does not cover the modifier"
    );
    for hotkey in [HOTKEY, UNREGISTERED] {
        let error = WeightPreflight::fetch(&best, NetUid(1), MechId(0), hotkey)
            .await
            .expect_err("a miss would read as UID 0");
        let refusal = by_name_error(&error);
        assert_eq!(
            (refusal.item.as_str(), refusal.part.as_str()),
            ("Uids", "modifier")
        );
    }
    client.close().await;
    Ok(())
}

/// The client returns one result per read: a read that does not fit fails
/// alone, and the others share one storage request.
#[tokio::test]
async fn fetch_resolved_reports_each_read_separately() -> anyhow::Result<()> {
    let (chain, client) = fixture_with(|meta| {
        let signed = add_type(meta, &[], TypeDef::Primitive(TypeDefPrimitive::I64));
        retype(meta, "ActivityCutoffFactorMilli", signed, vec![0; 8]);
    })
    .await?;
    let best = client.at(BlockRef::Best).await?;
    let read = |name: &str| rtao_client::ResolvedRead {
        storage: *reads::VALIDATOR_READS
            .iter()
            .find(|item| item.name == name)
            .expect("validator read"),
        keys: vec![rtao_runtime::ReadDatum::Uint(1)],
    };
    let before = chain.requests().len();
    let values = best
        .fetch_resolved(vec![
            read("Tempo"),
            read("ActivityCutoffFactorMilli"),
            read("SubnetEpochIndex"),
        ])
        .await?;
    let keys = chain.requests()[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .map(|request| request.params[0].as_array().expect("keys").len())
        .collect::<Vec<_>>();
    assert_eq!(keys, [2]);
    assert_eq!(values.len(), 3);
    assert_eq!(
        values[0].as_ref().expect("Tempo"),
        &Some(rtao_runtime::ReadDatum::Uint(10))
    );
    assert!(matches!(
        &values[1],
        Err(rtao_client::ClientError::ReadResolveIncompatible(refusal))
            if refusal.item == "ActivityCutoffFactorMilli"
    ));
    assert_eq!(
        values[2].as_ref().expect("SubnetEpochIndex"),
        &Some(rtao_runtime::ReadDatum::Uint(5))
    );
    client.close().await;
    Ok(())
}
