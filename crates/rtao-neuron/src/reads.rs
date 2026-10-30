//! Neuron storage reads after a runtime change (D-089).
//!
//! The validator run path reads its storage through compiled typed
//! descriptors. Each read routes like weight call composition (D-088): the
//! compiled read runs unchanged while the entry's exact shape still matches.
//! Otherwise the entry is resolved by name against live metadata through
//! [`rtao_runtime::ResolvedStorage`], with key and value classes from
//! [`VALIDATOR_READS`], and the decoded [`ReadDatum`]s are converted into the
//! same Rust values with range checks. A change whose meaning cannot be
//! recovered refuses only the reads that need that entry, with a
//! [`rtao_client::ClientError::ReadResolveIncompatible`] naming the entry and
//! field.

use rtao_client::{ClientError, Fetch, FromStorage, ResolvedRead, Snapshot, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, DefaultAccount, MechId, NetUid, NetUidStorageIndex};
use rtao_runtime::{
    ReadClass, ReadDatum, ReadField, ReadPresence, ResolvedStorage, RuntimeHandle,
    StorageDescriptor, StorageQuery, StorageSurface,
};
use std::collections::BTreeSet;
use std::sync::Mutex;

const SUBTENSOR: &str = "SubtensorModule";
const NET: &[ReadClass] = &[ReadClass::Uint(16)];

const fn subnet(name: &'static str, value: ReadClass) -> ResolvedStorage {
    ResolvedStorage {
        pallet: SUBTENSOR,
        name,
        keys: NET,
        value,
        presence: ReadPresence::Default,
    }
}

const fn global(pallet: &'static str, name: &'static str, value: ReadClass) -> ResolvedStorage {
    ResolvedStorage {
        pallet,
        name,
        keys: &[],
        value,
        presence: ReadPresence::Default,
    }
}

/// Fields of one `TimelockedWeightCommits` queue entry. Alternative names
/// are the ones the runtime's own reveal code and commit call use for the
/// same values, so a tuple that becomes a struct keeps its meaning.
pub const TIMELOCKED_COMMIT_FIELDS: &[ReadField] = &[
    ReadField {
        names: &["who", "account", "hotkey"],
        class: ReadClass::Account,
    },
    ReadField {
        names: &["commit_block"],
        class: ReadClass::Uint(64),
    },
    ReadField {
        names: &[
            "commit",
            "serialized_compressed_commit",
            "serialized_compresssed_commit",
        ],
        class: ReadClass::Bytes,
    },
    ReadField {
        names: &["reveal_round", "round_number"],
        class: ReadClass::Uint(64),
    },
];

/// Name-resolved form of every validator run-path storage read: the
/// `weights::PREFLIGHT_STORAGE` entries followed by the
/// `weights::RUN_PATH_STORAGE` entries, in the same order.
pub const VALIDATOR_READS: [ResolvedStorage; 23] = [
    ResolvedStorage {
        pallet: SUBTENSOR,
        name: "Uids",
        keys: &[ReadClass::Uint(16), ReadClass::Account],
        value: ReadClass::Uint(16),
        presence: ReadPresence::Optional,
    },
    subnet("LastUpdate", ReadClass::Seq(&ReadClass::Uint(64))),
    subnet("WeightsSetRateLimit", ReadClass::Uint(64)),
    subnet("CommitRevealWeightsEnabled", ReadClass::Bool),
    global(SUBTENSOR, "CommitRevealWeightsVersion", ReadClass::Uint(16)),
    subnet("RevealPeriodEpochs", ReadClass::Uint(64)),
    subnet("WeightsVersionKey", ReadClass::Uint(64)),
    subnet("MinAllowedWeights", ReadClass::Uint(16)),
    subnet("MaxWeightsLimit", ReadClass::Uint(16)),
    subnet("SubnetworkN", ReadClass::Uint(16)),
    subnet(
        "SubnetEpochConsensus",
        ReadClass::Variants(&["Yuma", "Null"]),
    ),
    subnet("ValidatorPermit", ReadClass::Seq(&ReadClass::Bool)),
    subnet("Tempo", ReadClass::Uint(16)),
    subnet("LastEpochBlock", ReadClass::Uint(64)),
    subnet("PendingEpochAt", ReadClass::Uint(64)),
    subnet("SubnetEpochIndex", ReadClass::Uint(64)),
    subnet("BlocksSinceLastStep", ReadClass::Uint(64)),
    subnet("MechanismCountCurrent", ReadClass::Uint(8)),
    subnet("ActivityCutoffFactorMilli", ReadClass::Uint(32)),
    global(SUBTENSOR, "StakeThreshold", ReadClass::Uint(64)),
    subnet("SubnetOwnerHotkey", ReadClass::Account),
    ResolvedStorage {
        pallet: SUBTENSOR,
        name: "TimelockedWeightCommits",
        keys: &[ReadClass::Uint(16), ReadClass::Uint(64)],
        value: ReadClass::Seq(&ReadClass::Record(TIMELOCKED_COMMIT_FIELDS)),
        presence: ReadPresence::Default,
    },
    global("Drand", "LastStoredRound", ReadClass::Uint(64)),
];

/// Class of the Aura `SlotDuration` constant, in milliseconds.
#[cfg(feature = "validator")]
pub const SLOT_DURATION: ReadClass = ReadClass::Uint(64);

/// The name-resolved form of a validator storage read.
pub(crate) fn resolved_storage(surface: &StorageSurface) -> Option<&'static ResolvedStorage> {
    VALIDATOR_READS
        .iter()
        .find(|item| item.pallet == surface.pallet && item.name == surface.name)
}

/// How one validator storage read runs under a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The compiled typed read; its exact shape matches.
    Compiled,
    /// Resolved by name against live metadata.
    Resolved,
}

fn storage_id(surface: &StorageSurface) -> String {
    format!("storage:{}.{}", surface.pallet, surface.name)
}

fn changed(runtime: &RuntimeHandle, id: &str) -> String {
    let detail = runtime
        .surface_report()
        .changes
        .iter()
        .filter(|change| change.descriptor == id)
        .map(|change| change.detail.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if detail.is_empty() {
        "not verified by this rtao build".to_owned()
    } else {
        detail
    }
}

/// Select how a storage read runs under `runtime`.
///
/// An entry whose absence is meaningful also needs its live modifier on the
/// compiled route, which the exact shape hash does not cover. An entry
/// outside [`VALIDATOR_READS`] keeps the exact gate.
pub(crate) fn route(
    runtime: &RuntimeHandle,
    surface: &StorageSurface,
) -> Result<Route, ClientError> {
    let id = storage_id(surface);
    let item = resolved_storage(surface);
    if runtime.surface_report().shape_matches(&id) {
        if let Some(item) = item {
            item.check_presence(runtime)?;
        }
        return Ok(Route::Compiled);
    }
    let Some(item) = item else {
        return Err(ClientError::SurfaceChanged {
            descriptor: surface.name,
            detail: changed(runtime, &id),
        });
    };
    item.check(runtime)?;
    Ok(Route::Resolved)
}

/// Runtimes and entries already reported as read by name.
static ADVISED: Mutex<BTreeSet<(rtao_runtime::RuntimeStamp, &'static str, &'static str)>> =
    Mutex::new(BTreeSet::new());

/// Warn once per runtime and entry that a read now resolves by name.
fn advise(runtime: &RuntimeHandle, surface: &StorageSurface) {
    let fresh = ADVISED
        .lock()
        .map(|mut advised| advised.insert((runtime.stamp(), surface.pallet, surface.name)))
        .unwrap_or(true);
    if fresh {
        tracing::warn!(
            storage = %format!("{}.{}", surface.pallet, surface.name),
            spec_version = runtime.spec_version(),
            change = %changed(runtime, &storage_id(surface)),
            "the compiled storage descriptor no longer matches the live runtime; reading it by name against live metadata"
        );
    }
}

/// Route every surface; true when any needs the name-resolved read.
fn needs_resolution(
    runtime: &RuntimeHandle,
    surfaces: &[StorageSurface],
) -> Result<bool, ClientError> {
    let mut resolved = false;
    for surface in surfaces {
        if route(runtime, surface)? == Route::Resolved {
            advise(runtime, surface);
            resolved = true;
        }
    }
    Ok(resolved)
}

/// Conversion of a decoded datum into the Rust value a typed read returns.
pub(crate) trait FromDatum: Sized {
    /// Convert, refusing a datum of another class or out of range.
    fn from_datum(datum: ReadDatum) -> Result<Self, String>;
}

fn uint<T: TryFrom<u128>>(datum: &ReadDatum) -> Result<T, String> {
    let value = datum
        .as_uint()
        .ok_or_else(|| format!("expected an integer, decoded {datum:?}"))?;
    T::try_from(value).map_err(|_| format!("integer {value} is out of range"))
}

macro_rules! uint_datum {
    ($($ty:ty => $make:expr),* $(,)?) => {$(
        impl FromDatum for $ty {
            fn from_datum(datum: ReadDatum) -> Result<Self, String> {
                uint(&datum).map($make)
            }
        }
    )*};
}

uint_datum!(
    u16 => |value: u16| value,
    u32 => |value: u32| value,
    u64 => |value: u64| value,
    MechId => MechId,
);

impl FromDatum for bool {
    fn from_datum(datum: ReadDatum) -> Result<Self, String> {
        datum
            .as_bool()
            .ok_or_else(|| format!("expected a bool, decoded {datum:?}"))
    }
}

impl FromDatum for AccountId {
    fn from_datum(datum: ReadDatum) -> Result<Self, String> {
        datum
            .as_account()
            .map(AccountId)
            .ok_or_else(|| format!("expected an account, decoded {datum:?}"))
    }
}

impl FromDatum for DefaultAccount {
    fn from_datum(datum: ReadDatum) -> Result<Self, String> {
        AccountId::from_datum(datum).map(DefaultAccount::new)
    }
}

impl<T: FromDatum> FromDatum for Vec<T> {
    fn from_datum(datum: ReadDatum) -> Result<Self, String> {
        let ReadDatum::Seq(items) = datum else {
            return Err(format!("expected a sequence, decoded {datum:?}"));
        };
        items.into_iter().map(T::from_datum).collect()
    }
}

impl FromDatum for (AccountId, u64, Vec<u8>, u64) {
    fn from_datum(datum: ReadDatum) -> Result<Self, String> {
        match datum {
            ReadDatum::Record(fields) => match <[ReadDatum; 4]>::try_from(fields) {
                Ok([who, block, ReadDatum::Bytes(commit), round]) => Ok((
                    AccountId::from_datum(who)?,
                    u64::from_datum(block)?,
                    commit,
                    u64::from_datum(round)?,
                )),
                Ok(fields) => Err(format!("unexpected commit fields {fields:?}")),
                Err(fields) => Err(format!(
                    "expected 4 commit fields, decoded {}",
                    fields.len()
                )),
            },
            other => Err(format!("expected a commit record, decoded {other:?}")),
        }
    }
}

/// An enum read keeps the dynamic shape the compiled read returns: a unit
/// variant is its name.
impl FromDatum for DynValue {
    fn from_datum(datum: ReadDatum) -> Result<Self, String> {
        datum_value(datum).map(DynValue::new)
    }
}

/// The core value a dynamic compiled read returns for the same data.
fn datum_value(datum: ReadDatum) -> Result<Value, String> {
    Ok(match datum {
        ReadDatum::Uint(value) => i128::try_from(value).map_or(Value::Uint(value), Value::Int),
        ReadDatum::Bool(value) => Value::Bool(value),
        ReadDatum::Variant(name) => Value::Str(name),
        ReadDatum::Seq(items) => Value::List(
            items
                .into_iter()
                .map(datum_value)
                .collect::<Result<_, _>>()?,
        ),
        other => return Err(format!("no dynamic form for {other:?}")),
    })
}

/// The output of one typed storage read, built from a name-resolved datum.
pub(crate) trait Resolve: FromStorage {
    /// Convert; `None` is an absent optional entry.
    fn resolve(datum: Option<ReadDatum>) -> Result<Self::Out, String>;
}

impl<T: FromDatum + rtao_client::ReadValue> Resolve for T {
    fn resolve(datum: Option<ReadDatum>) -> Result<T, String> {
        T::from_datum(datum.ok_or("the entry read as absent")?)
    }
}

impl<T: FromDatum + rtao_client::ReadValue> Resolve for rtao_runtime::OptionalStorage<T> {
    fn resolve(datum: Option<ReadDatum>) -> Result<Option<T>, String> {
        datum.map(T::from_datum).transpose()
    }
}

/// One unhashed storage key component as a datum.
pub(crate) trait KeyDatum {
    /// The datum the key class encodes.
    fn key_datum(&self) -> ReadDatum;
}

impl KeyDatum for NetUid {
    fn key_datum(&self) -> ReadDatum {
        ReadDatum::Uint(self.0.into())
    }
}
impl KeyDatum for NetUidStorageIndex {
    fn key_datum(&self) -> ReadDatum {
        ReadDatum::Uint(self.0.into())
    }
}
impl KeyDatum for u64 {
    fn key_datum(&self) -> ReadDatum {
        ReadDatum::Uint((*self).into())
    }
}
impl KeyDatum for AccountId {
    fn key_datum(&self) -> ReadDatum {
        ReadDatum::Account(self.0)
    }
}

/// Unhashed keys of a typed query as datums.
pub(crate) trait KeyData {
    /// Key datums in metadata order.
    fn key_data(&self) -> Vec<ReadDatum>;
}
impl KeyData for () {
    fn key_data(&self) -> Vec<ReadDatum> {
        Vec::new()
    }
}
impl<A: KeyDatum> KeyData for (A,) {
    fn key_data(&self) -> Vec<ReadDatum> {
        vec![self.0.key_datum()]
    }
}
impl<A: KeyDatum, B: KeyDatum> KeyData for (A, B) {
    fn key_data(&self) -> Vec<ReadDatum> {
        vec![self.0.key_datum(), self.1.key_datum()]
    }
}

/// A typed storage plan that can also be read by name.
pub(crate) trait ResolvedFetch: Fetch {
    /// Each query's surface and key datums, in plan order.
    fn reads(&self) -> Vec<(StorageSurface, Vec<ReadDatum>)>;
    /// Build the typed output from per-read results in plan order.
    fn take(
        self,
        values: &mut dyn Iterator<Item = Result<Option<ReadDatum>, ClientError>>,
    ) -> anyhow::Result<Self::Output>;
}

impl<K, V> ResolvedFetch for StorageQuery<K, V>
where
    K: rtao_client::KeyArgs + KeyData + 'static,
    V: Resolve,
{
    fn reads(&self) -> Vec<(StorageSurface, Vec<ReadDatum>)> {
        vec![(self.descriptor.surface(), self.keys.key_data())]
    }
    fn take(
        self,
        values: &mut dyn Iterator<Item = Result<Option<ReadDatum>, ClientError>>,
    ) -> anyhow::Result<V::Out> {
        let surface = self.descriptor.surface();
        let datum = values
            .next()
            .ok_or_else(|| anyhow::anyhow!("name-resolved storage result was omitted"))??;
        V::resolve(datum).map_err(|detail| {
            ClientError::from(rtao_runtime::ReadResolveError {
                pallet: surface.pallet.into(),
                item: surface.name.into(),
                part: "value".into(),
                detail,
            })
            .into()
        })
    }
}

macro_rules! resolved_tuples {
    ($(($($name:ident:$index:tt),+)),+ $(,)?) => {$(
        impl<$($name: ResolvedFetch),+> ResolvedFetch for ($($name,)+) {
            fn reads(&self) -> Vec<(StorageSurface, Vec<ReadDatum>)> {
                let mut reads = Vec::new();
                $(reads.extend(self.$index.reads());)+
                reads
            }
            fn take(
                self,
                values: &mut dyn Iterator<Item = Result<Option<ReadDatum>, ClientError>>,
            ) -> anyhow::Result<Self::Output> {
                Ok(($(self.$index.take(values)?,)+))
            }
        }
    )+};
}

resolved_tuples!(
    (A:0, B:1),
    (A:0, B:1, C:2),
    (A:0, B:1, C:2, D:3),
    (A:0, B:1, C:2, D:3, E:4),
    (A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7),
);

/// Read typed storage queries coalesced at `snapshot`, by name when any
/// entry's compiled shape no longer matches.
pub(crate) async fn fetch<F: ResolvedFetch>(
    snapshot: &Snapshot,
    plan: F,
) -> anyhow::Result<F::Output> {
    let runtime = snapshot.block().runtime.clone();
    let surfaces = plan
        .reads()
        .into_iter()
        .map(|(surface, _)| surface)
        .collect::<Vec<_>>();
    if !needs_resolution(&runtime, &surfaces)? {
        return Ok(snapshot.fetch(plan).await?);
    }
    fetch_by_name(snapshot, plan).await
}

/// The name-resolved half of [`fetch`], whatever the compiled shapes.
pub(crate) async fn fetch_by_name<F: ResolvedFetch>(
    snapshot: &Snapshot,
    plan: F,
) -> anyhow::Result<F::Output> {
    let reads = resolved_reads(plan.reads())?;
    let values = snapshot.fetch_resolved(reads).await?;
    let mut values = values.into_iter();
    let output = plan.take(&mut values)?;
    anyhow::ensure!(
        values.next().is_none(),
        "name-resolved storage returned extra values"
    );
    Ok(output)
}

/// [`fetch`] for one typed storage entry.
pub(crate) async fn storage<K, V>(
    snapshot: &Snapshot,
    descriptor: &StorageDescriptor<K, V>,
    key: K,
) -> anyhow::Result<V::Out>
where
    K: rtao_client::KeyArgs + KeyData + 'static,
    V: Resolve,
{
    fetch(snapshot, descriptor.at(key)).await
}

fn resolved_reads(
    reads: Vec<(StorageSurface, Vec<ReadDatum>)>,
) -> Result<Vec<ResolvedRead>, ClientError> {
    reads
        .into_iter()
        .map(|(surface, keys)| {
            resolved_storage(&surface)
                .map(|storage| ResolvedRead {
                    storage: *storage,
                    keys,
                })
                .ok_or_else(|| ClientError::SurfaceChanged {
                    descriptor: surface.name,
                    detail: "storage has no name-resolved form".into(),
                })
        })
        .collect()
}

/// One storage read of a dynamic plan: surface, core key values for the
/// compiled read and the same keys as datums.
#[cfg(feature = "validator")]
pub(crate) type DynRead = (StorageSurface, Vec<Value>, Vec<ReadDatum>);

/// Read a dynamic plan coalesced at `snapshot`, returning the values a
/// compiled [`rtao_client::ReadPlan`] returns. Name-resolved values take the same
/// dynamic shapes: integers, booleans, lists, unit variant names, and
/// `Null` for an absent optional entry.
#[cfg(feature = "validator")]
pub(crate) async fn fetch_dyn(
    snapshot: &Snapshot,
    reads: Vec<DynRead>,
) -> anyhow::Result<Vec<DynValue>> {
    let runtime = snapshot.block().runtime.clone();
    let surfaces = reads
        .iter()
        .map(|(surface, _, _)| *surface)
        .collect::<Vec<_>>();
    if !needs_resolution(&runtime, &surfaces)? {
        let mut plan = rtao_client::ReadPlan::new();
        for (surface, keys, _) in reads {
            plan.push(surface, keys);
        }
        return Ok(snapshot.fetch(plan).await?);
    }
    fetch_dyn_by_name(snapshot, reads).await
}

/// The name-resolved half of [`fetch_dyn`], whatever the compiled shapes.
#[cfg(feature = "validator")]
pub(crate) async fn fetch_dyn_by_name(
    snapshot: &Snapshot,
    reads: Vec<DynRead>,
) -> anyhow::Result<Vec<DynValue>> {
    let reads = resolved_reads(
        reads
            .into_iter()
            .map(|(surface, _, keys)| (surface, keys))
            .collect(),
    )?;
    let names = reads
        .iter()
        .map(|read| (read.storage.pallet, read.storage.name))
        .collect::<Vec<_>>();
    snapshot
        .fetch_resolved(reads)
        .await?
        .into_iter()
        .zip(names)
        .map(|(value, (pallet, item))| {
            let value = match value? {
                None => Value::Null,
                Some(datum) => datum_value(datum).map_err(|detail| {
                    ClientError::from(rtao_runtime::ReadResolveError {
                        pallet: pallet.into(),
                        item: item.into(),
                        part: "value".into(),
                        detail,
                    })
                })?,
            };
            Ok(DynValue::new(value))
        })
        .collect()
}

/// Aura's slot duration in milliseconds at `snapshot`, by name when the
/// compiled constant's shape no longer matches.
#[cfg(feature = "validator")]
pub(crate) async fn slot_duration(snapshot: &Snapshot) -> anyhow::Result<u64> {
    let surface = rtao_runtime::constants::aura::SLOT_DURATION.surface();
    let runtime = &snapshot.block().runtime;
    let id = format!("constant:{}.{}", surface.pallet, surface.name);
    if runtime.surface_report().shape_matches(&id) {
        return Ok(snapshot
            .constant(&rtao_runtime::constants::aura::SLOT_DURATION)
            .await?);
    }
    let datum = rtao_runtime::read_constant(runtime, surface.pallet, surface.name, &SLOT_DURATION)
        .map_err(ClientError::from)?;
    u64::from_datum(datum).map_err(|detail| anyhow::anyhow!("Aura.SlotDuration: {detail}"))
}
