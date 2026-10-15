// Derived from RaoFoundation/subtensor sdk/python/bittensor/metagraph.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! One metagraph projection shared by borrowed, typed and dynamic decoders.

use std::{
    collections::{BTreeMap, HashMap},
    sync::OnceLock,
};

use bytes::Bytes;
use rtao_core::codec::{Value, decode::Cursor, value::to_corpus_json};
use rtao_primitives::{AccountId, DynBalance, H256, MechId, NetUid, StakeWeight, Uid};
use rtao_reads::commitments::{CommitmentMap, NeuronCommitment};
use rtao_runtime::{
    RuntimeHandle, SurfaceDescriptor,
    models::{MetagraphWire, SelectiveMetagraphWire, ToDynamic},
    shapes,
};
use scale_decode::DecodeAsType;
use serde_json::{Value as Json, json};

use crate::{MetagraphError, MetagraphView, Result, view::BorrowedColumns};

/// Decoder used for the current snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodePath {
    /// Exact-layout positional parser with borrowed fixed-width columns.
    View,
    /// Named-field typed model after its projection contract passes.
    Typed,
    /// Metadata-directed core values, also the differential reference.
    Dynamic,
}

/// Decoder selection, including explicit paths for differential checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DecodePreference {
    /// Select the fastest shape-compatible path.
    #[default]
    Auto,
    /// Require the exact borrowed parser contract.
    View,
    /// Require the typed model's projection contract.
    Typed,
    /// Decode with the core's metadata-directed value decoder.
    Dynamic,
}

/// Options for a curated metagraph fetch.
#[derive(Clone, Copy, Debug)]
pub struct MetagraphOptions {
    /// Include live and revealed commitments at the metagraph's block.
    pub commitments: bool,
    /// Allow the borrowed parser, subject to the client and environment opt-outs.
    pub view: bool,
    /// Explicit decoder selection; requesting a disabled view is an error.
    pub decoder: DecodePreference,
}
impl Default for MetagraphOptions {
    fn default() -> Self {
        Self {
            commitments: true,
            view: std::env::var("RTAO_METAGRAPH_VIEW").as_deref() != Ok("0"),
            decoder: DecodePreference::Auto,
        }
    }
}

/// Chain identity of the state used to construct a metagraph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// Block height at which all graph and commitment reads were made.
    pub number: u64,
    /// Immutable block identity.
    pub hash: H256,
    /// Runtime spec used to decode the response.
    pub spec_version: u32,
}

/// Subnet identity, pool price and epoch information at one block.
#[derive(Clone, Debug, PartialEq)]
pub struct MetagraphHeader {
    /// Requested subnet, distinct from the mechanism's packed storage index.
    pub netuid: NetUid,
    /// Mechanism from the full response's packed `netuid`, or a selective request.
    pub mechid: MechId,
    /// Subnet name decoded with UTF-8 replacement, as Python does.
    pub name: String,
    /// Chain token symbol.
    pub symbol: String,
    /// Chain height reported by the runtime record.
    pub block: u64,
    /// Epoch tempo.
    pub tempo: u64,
    /// Last epoch step.
    pub last_step: u64,
    /// Blocks elapsed since the last step.
    pub blocks_since_last_step: u64,
    /// Subnet owner's hotkey.
    pub owner_hotkey: AccountId,
    /// Subnet owner's coldkey.
    pub owner_coldkey: AccountId,
    /// Block at which this subnet registered.
    pub network_registered_at: u64,
    /// Runtime's reported population, or row count when zero.
    pub num_uids: u64,
    /// Configured population limit.
    pub max_uids: u64,
    /// TAO reserve divided by alpha reserve; absent when alpha reserve is zero.
    pub price: Option<f64>,
    /// I96F32 moving price interpreted as TAO per alpha.
    pub moving_price: Option<f64>,
    /// Runtime-shaped subnet identity, when present.
    pub identity: Option<Json>,
}

#[derive(Debug)]
enum Columns {
    Borrowed(BorrowedColumns),
    Owned(Value),
}

/// A subnet's complete row projection, retaining the original response bytes.
#[derive(Debug)]
pub struct Metagraph {
    raw: Bytes,
    columns: Columns,
    fields: Value,
    runtime: Option<RuntimeHandle>,
    api_method: &'static str,
    path: DecodePath,
    hotkeys: OnceLock<Vec<AccountId>>,
    by_hotkey: OnceLock<HashMap<AccountId, Uid>>,
    #[cfg(feature = "persist")]
    pub(crate) persistence_origin: Option<crate::persist::Origin>,
    /// Subnet-level fields and economic terms.
    pub header: MetagraphHeader,
    /// Pinned state identity.
    pub at: SnapshotInfo,
    /// Registered commitments by UID; absent when commitment fetching was disabled.
    pub commitments: Option<BTreeMap<Uid, NeuronCommitment>>,
    /// Commitments belonging to hotkeys that are no longer registered.
    pub unregistered_commitments: Option<BTreeMap<String, NeuronCommitment>>,
}

impl Metagraph {
    /// Decode a selected mechanism graph by name, retaining the original record.
    /// Fields outside the selection use zero/empty values in the row projection;
    /// [`Self::runtime_value`] retains their original `None` values.
    pub fn decode_selective(
        raw: Bytes,
        runtime: &RuntimeHandle,
        netuid: NetUid,
        mechid: MechId,
        at: SnapshotInfo,
        options: MetagraphOptions,
    ) -> Result<Option<Self>> {
        if options.decoder == DecodePreference::View {
            return Err(MetagraphError::Shape(
                "selective graphs have no positional view".into(),
            ));
        }
        let name = "get_selective_mechagraph";
        let method = runtime
            .runtime_api("SubnetInfoRuntimeApi", name)
            .ok_or_else(|| MetagraphError::Shape("get_selective_mechagraph is absent".into()))?;
        let descriptor =
            rtao_runtime::runtime_apis::subnet_info_runtime_api::GET_SELECTIVE_MECHAGRAPH.surface();
        let compatible = rtao_runtime::verify_surface_with(
            runtime,
            &[SurfaceDescriptor::RuntimeApi(descriptor)],
        )
        .shapes_match();
        let (fields, path) = if options.decoder != DecodePreference::Dynamic && compatible {
            let mut input = raw.as_ref();
            let model = Option::<SelectiveMetagraphWire>::decode_as_type(
                &mut input,
                method.output,
                &runtime.core().types,
            )?;
            if !input.is_empty() {
                return Err(MetagraphError::TrailingBytes);
            }
            let Some(model) = model else { return Ok(None) };
            (model.to_dynamic(), DecodePath::Typed)
        } else {
            if options.decoder == DecodePreference::Typed {
                return Err(MetagraphError::Shape(
                    "selective metagraph projection changed".into(),
                ));
            }
            let mut cursor = Cursor::new(&raw);
            let fields = runtime.core().decode_id(method.output, &mut cursor)?;
            if cursor.remaining() != 0 {
                return Err(MetagraphError::TrailingBytes);
            }
            if matches!(fields, Value::Null) {
                return Ok(None);
            }
            (fields, DecodePath::Dynamic)
        };
        let fields = selective_defaults(fields, netuid, mechid, at)?;
        let mut graph = Self::build(
            raw,
            Columns::Owned(fields.clone()),
            fields,
            Some((runtime.clone(), name)),
            path,
            netuid,
            at,
        )?;
        // The selective mechanism API always emits the base netuid, unlike
        // get_mechagraph's packed netuid. Mechanism identity comes from its input.
        graph.header.mechid = mechid;
        Ok(Some(graph))
    }

    /// Decode a complete runtime-API response, falling back when shape gates differ.
    pub fn decode(
        raw: Bytes,
        runtime: &RuntimeHandle,
        netuid: NetUid,
        at: SnapshotInfo,
        options: MetagraphOptions,
    ) -> Result<Option<Self>> {
        Self::decode_full(raw, runtime, netuid, at, options, "get_metagraph")
    }

    pub(crate) fn decode_mechanism(
        raw: Bytes,
        runtime: &RuntimeHandle,
        netuid: NetUid,
        at: SnapshotInfo,
        mut options: MetagraphOptions,
    ) -> Result<Option<Self>> {
        options.view = false;
        if options.decoder == DecodePreference::View {
            return Err(MetagraphError::Shape(
                "mechanism graphs have no positional view".into(),
            ));
        }
        Self::decode_full(raw, runtime, netuid, at, options, "get_mechagraph")
    }

    fn decode_full(
        raw: Bytes,
        runtime: &RuntimeHandle,
        netuid: NetUid,
        at: SnapshotInfo,
        mut options: MetagraphOptions,
        name: &'static str,
    ) -> Result<Option<Self>> {
        options.view &= std::env::var("RTAO_METAGRAPH_VIEW").as_deref() != Ok("0");
        if !options.view && options.decoder == DecodePreference::View {
            return Err(MetagraphError::Shape("metagraph view is disabled".into()));
        }
        let method = runtime
            .runtime_api("SubnetInfoRuntimeApi", name)
            .ok_or_else(|| MetagraphError::Shape("get_metagraph is absent".into()))?;
        if options.decoder == DecodePreference::View
            || (options.decoder == DecodePreference::Auto && options.view)
        {
            match MetagraphView::parse(&raw, runtime) {
                Ok(None) => return Ok(None),
                Ok(Some(view)) => {
                    let fields = Value::record(view.fields.clone());
                    let columns = view.columns;
                    return Self::build(
                        raw,
                        Columns::Borrowed(columns),
                        fields,
                        Some((runtime.clone(), name)),
                        DecodePath::View,
                        netuid,
                        at,
                    )
                    .map(Some);
                }
                Err(MetagraphError::Shape(_)) if options.decoder == DecodePreference::Auto => {}
                Err(error) => return Err(error),
            }
        }
        if options.decoder == DecodePreference::Typed || options.decoder == DecodePreference::Auto {
            if typed_gate(runtime, name)? {
                let mut input = raw.as_ref();
                let model = Option::<MetagraphWire>::decode_as_type(
                    &mut input,
                    method.output,
                    &runtime.core().types,
                )?;
                if !input.is_empty() {
                    return Err(MetagraphError::TrailingBytes);
                }
                let Some(model) = model else {
                    return Ok(None);
                };
                let fields = model.to_dynamic();
                return Self::build(
                    raw,
                    Columns::Owned(fields.clone()),
                    fields,
                    Some((runtime.clone(), name)),
                    DecodePath::Typed,
                    netuid,
                    at,
                )
                .map(Some);
            } else if options.decoder == DecodePreference::Typed {
                return Err(MetagraphError::Shape("metagraph projection changed".into()));
            }
        }
        let mut cursor = Cursor::new(&raw);
        let fields = runtime.core().decode_id(method.output, &mut cursor)?;
        if cursor.remaining() != 0 {
            return Err(MetagraphError::TrailingBytes);
        }
        if matches!(fields, Value::Null) {
            return Ok(None);
        }
        Self::build(
            raw,
            Columns::Owned(fields.clone()),
            fields,
            Some((runtime.clone(), name)),
            DecodePath::Dynamic,
            netuid,
            at,
        )
        .map(Some)
    }

    /// Project Python's dict-shaped runtime value, including padded and old columns.
    pub fn from_dynamic(netuid: NetUid, graph: Value, at: SnapshotInfo) -> Result<Self> {
        Self::build(
            Bytes::new(),
            Columns::Owned(graph.clone()),
            graph,
            None,
            DecodePath::Dynamic,
            netuid,
            at,
        )
    }

    fn build(
        raw: Bytes,
        columns: Columns,
        fields: Value,
        source: Option<(RuntimeHandle, &'static str)>,
        path: DecodePath,
        netuid: NetUid,
        at: SnapshotInfo,
    ) -> Result<Self> {
        let (runtime, api_method) = source.map_or((None, "get_metagraph"), |(runtime, method)| {
            (Some(runtime), method)
        });
        if !matches!(fields, Value::Dict(_)) {
            return Err(MetagraphError::Malformed(
                "metagraph is not a record".into(),
            ));
        }
        let len = match &columns {
            Columns::Borrowed(columns) => columns
                .fixed
                .get("hotkeys")
                .map_or(0, |(range, width)| range.len() / width),
            Columns::Owned(fields) => list(field(fields, "hotkeys")).len(),
        };
        if len > usize::from(u16::MAX) + 1 {
            return Err(MetagraphError::Malformed(
                "neuron count exceeds UID range".into(),
            ));
        }
        let alpha_in = integer(field(&fields, "alpha_in"))?;
        let tao_in = integer(field(&fields, "tao_in"))?;
        let packed = integer(field(&fields, "netuid"))?;
        let moving_bits = field(field(&fields, "moving_price"), "bits");
        if path == DecodePath::Dynamic
            && matches!(moving_bits, Value::Int(_) | Value::Uint(_))
            && let Some(runtime) = &runtime
        {
            check_moving_price_type(runtime, api_method)?;
        }
        let moving_price = match moving_bits {
            Value::Int(bits) => rtao_primitives::ratio::integer_division(*bits, 4_294_967_296),
            Value::Uint(bits) => rtao_primitives::ratio::integer_division(
                i128::try_from(*bits).map_err(|_| {
                    MetagraphError::Malformed("moving price exceeds signed I96F32".into())
                })?,
                4_294_967_296,
            ),
            _ => None,
        };
        let header = MetagraphHeader {
            netuid,
            mechid: MechId(
                u8::try_from(
                    (if packed == 0 {
                        u64::from(netuid.0)
                    } else {
                        packed
                    }) / 4096,
                )
                .map_err(|_| MetagraphError::Malformed("mechanism index exceeds u8".into()))?,
            ),
            name: text(field(&fields, "name"))?,
            symbol: text(field(&fields, "symbol"))?,
            block: required_integer(&fields, "block")?,
            tempo: required_integer(&fields, "tempo")?,
            last_step: required_integer(&fields, "last_step")?,
            blocks_since_last_step: required_integer(&fields, "blocks_since_last_step")?,
            owner_hotkey: account(field(&fields, "owner_hotkey"))?,
            owner_coldkey: account(field(&fields, "owner_coldkey"))?,
            network_registered_at: required_integer(&fields, "network_registered_at")?,
            num_uids: match integer(field(&fields, "num_uids"))? {
                0 => len as u64,
                other => other,
            },
            max_uids: integer(field(&fields, "max_uids"))?,
            price: rtao_primitives::ratio::integer_division(i128::from(tao_in), alpha_in),
            moving_price,
            identity: record_json(field(&fields, "identity")),
        };
        let graph = Self {
            raw,
            columns,
            fields,
            runtime,
            api_method,
            path,
            hotkeys: OnceLock::new(),
            by_hotkey: OnceLock::new(),
            #[cfg(feature = "persist")]
            persistence_origin: None,
            header,
            at,
            commitments: None,
            unregistered_commitments: None,
        };
        // Fixed-width borrowed keys are already validated by the parser's bounds.
        if matches!(graph.columns, Columns::Owned(_)) {
            for uid in 0..len {
                account(&graph.value("hotkeys", uid))?;
            }
        }
        Ok(graph)
    }

    /// Original SCALE response bytes, retained for persistence and re-decoding.
    pub fn raw(&self) -> &Bytes {
        &self.raw
    }
    #[cfg(feature = "persist")]
    pub(crate) fn set_persistence_origin(&mut self, genesis: H256, source: crate::persist::Source) {
        self.persistence_origin = Some(crate::persist::Origin {
            genesis,
            source,
            at: self.at,
            netuid: self.header.netuid,
            mechid: self.header.mechid,
        });
    }
    pub(crate) fn source_netuid(&self) -> Result<NetUid> {
        let packed = integer(field(&self.fields, "netuid"))?;
        let packed = u16::try_from(packed)
            .map_err(|_| MetagraphError::Malformed("response netuid exceeds u16".into()))?;
        Ok(rtao_primitives::NetUidStorageIndex(packed).split().0)
    }
    /// Decoder selected for this snapshot.
    pub fn decode_path(&self) -> DecodePath {
        self.path
    }
    /// Associate a pinned commitment map with registered rows and leftover hotkeys.
    pub fn assign_commitments(&mut self, commitments: CommitmentMap) {
        let mut registered = BTreeMap::new();
        let mut unregistered = BTreeMap::new();
        for (hotkey, mut commitment) in commitments.into_entries() {
            let uid = hotkey
                .parse::<AccountId>()
                .ok()
                .and_then(|key| self.by_hotkey(&key).map(|row| row.uid));
            if let Some(uid) = uid {
                commitment.uid = Some(uid);
                registered.insert(uid, commitment);
            } else {
                commitment.uid = None;
                unregistered.insert(hotkey, commitment);
            }
        }
        self.commitments = Some(registered);
        self.unregistered_commitments = Some(unregistered);
    }
    /// Number of registered hotkey rows, matching Python's row count.
    pub fn len(&self) -> usize {
        match &self.columns {
            Columns::Borrowed(columns) => columns
                .fixed
                .get("hotkeys")
                .map_or(0, |(range, width)| range.len() / width),
            Columns::Owned(fields) => list(field(fields, "hotkeys")).len(),
        }
    }
    /// Whether the graph has no registered neurons.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Borrow an indexed hotkey. Materializes account wrappers only on first use.
    pub fn hotkey(&self, uid: Uid) -> Option<&AccountId> {
        self.hotkeys
            .get_or_init(|| {
                (0..self.len())
                    .filter_map(|uid| self.key_at("hotkeys", uid).ok().flatten())
                    .collect()
            })
            .get(usize::from(uid.0))
    }
    /// Borrow a neuron row, or return `None` for an unknown UID.
    pub fn neuron(&self, uid: Uid) -> Option<NeuronRef<'_>> {
        (usize::from(uid.0) < self.len()).then_some(NeuronRef { graph: self, uid })
    }
    /// Find a registered caller using the lazily built constant-time index.
    pub fn by_hotkey(&self, hotkey: &AccountId) -> Option<NeuronRef<'_>> {
        let index = self.by_hotkey.get_or_init(|| {
            let mut index = HashMap::with_capacity(self.len());
            for uid in 0..self.len() {
                if let Ok(uid) = u16::try_from(uid)
                    && let Some(hotkey) = self.hotkey(Uid(uid))
                {
                    index.entry(*hotkey).or_insert(Uid(uid));
                }
            }
            index
        });
        index.get(hotkey).and_then(|uid| self.neuron(*uid))
    }
    /// Iterate neurons holding a validator permit.
    pub fn validators(&self) -> impl Iterator<Item = NeuronRef<'_>> {
        (0..self.len())
            .filter_map(|uid| {
                u16::try_from(uid)
                    .ok()
                    .and_then(|uid| self.neuron(Uid(uid)))
            })
            .filter(|neuron| neuron.validator_permit())
    }
    /// Iterate served HTTP endpoints with their UID and receiver hotkey.
    pub fn axon_targets(&self) -> impl Iterator<Item = rtao_primitives::axon::AxonTarget> + '_ {
        (0..self.len()).filter_map(|uid| {
            let uid = Uid(u16::try_from(uid).ok()?);
            let axon = self.value("axons", usize::from(uid.0));
            let ip = match field(&axon, "ip") {
                Value::Int(ip) => u128::try_from(*ip).ok()?,
                Value::Uint(ip) => *ip,
                _ => return None,
            };
            let ip_type = u8::try_from(integer(field(&axon, "ip_type")).ok()?).ok()?;
            let port = u16::try_from(integer(field(&axon, "port")).ok()?).ok()?;
            Some(rtao_primitives::axon::AxonTarget {
                uid,
                hotkey: *self.hotkey(uid)?,
                base_url: rtao_primitives::axon::axon_base_url(ip, ip_type, port)?,
            })
        })
    }
    /// Materialize Python's complete ordered neuron rows.
    pub fn to_owned_neurons(&self) -> Result<Vec<MetagraphNeuron>> {
        (0..self.len()).map(|uid| self.owned_neuron(uid)).collect()
    }
    /// Runtime-shaped record, preserving fields appended beyond the typed projection.
    pub fn runtime_value(&self) -> Result<Value> {
        if let Some(runtime) = &self.runtime {
            let method = runtime
                .runtime_api("SubnetInfoRuntimeApi", self.api_method)
                .ok_or_else(|| MetagraphError::Shape("get_metagraph is absent".into()))?;
            let mut cursor = Cursor::new(&self.raw);
            let value = runtime.core().decode_id(method.output, &mut cursor)?;
            if cursor.remaining() != 0 {
                return Err(MetagraphError::TrailingBytes);
            }
            Ok(value)
        } else {
            Ok(self.fields.clone())
        }
    }
    /// Python-compatible dataclass JSON, including the untouched runtime record.
    pub fn to_json(&self) -> Result<Json> {
        let header = &self.header;
        let neurons = self
            .to_owned_neurons()?
            .iter()
            .map(MetagraphNeuron::to_json)
            .collect::<Vec<_>>();
        let mut commitments = serde_json::Map::new();
        if let Some(entries) = &self.commitments {
            for (uid, commitment) in entries {
                commitments.insert(
                    uid.0.to_string(),
                    serde_json::to_value(commitment)
                        .map_err(|error| MetagraphError::Malformed(error.to_string()))?,
                );
            }
        }
        let mut unregistered = serde_json::Map::new();
        if let Some(entries) = &self.unregistered_commitments {
            for (key, commitment) in entries {
                unregistered.insert(
                    key.clone(),
                    serde_json::to_value(commitment)
                        .map_err(|error| MetagraphError::Malformed(error.to_string()))?,
                );
            }
        }
        Ok(
            json!({"netuid":header.netuid.0, "mechid":header.mechid.0, "name":header.name, "symbol":header.symbol,
            "block":header.block, "tempo":header.tempo, "last_step":header.last_step, "blocks_since_last_step":header.blocks_since_last_step,
            "owner_hotkey":header.owner_hotkey.to_string(), "owner_coldkey":header.owner_coldkey.to_string(),
            "network_registered_at":header.network_registered_at,"num_uids":header.num_uids,"max_uids":header.max_uids,
            "price":header.price,"moving_price":header.moving_price,"identity":header.identity,"neurons":neurons,
            "commitments":commitments, "unregistered_commitments":unregistered,
            "raw":to_corpus_json(&self.runtime_value()?)}),
        )
    }

    fn value(&self, name: &str, uid: usize) -> Value {
        match &self.columns {
            Columns::Borrowed(columns) => {
                columns.value(&self.raw, name, uid).unwrap_or(Value::Null)
            }
            Columns::Owned(fields) => list(field(fields, name))
                .get(uid)
                .cloned()
                .unwrap_or(Value::Null),
        }
    }
    fn key_at(&self, name: &str, uid: usize) -> Result<Option<AccountId>> {
        if let Columns::Borrowed(columns) = &self.columns {
            let Some((range, width)) = columns.fixed.get(name) else {
                return Ok(None);
            };
            let start = uid
                .checked_mul(*width)
                .and_then(|offset| range.start.checked_add(offset))
                .ok_or_else(|| {
                    MetagraphError::Malformed("account column index overflows".into())
                })?;
            let end = start.checked_add(*width).ok_or_else(|| {
                MetagraphError::Malformed("account column index overflows".into())
            })?;
            if end > range.end {
                return Ok(None);
            }
            let bytes = self
                .raw
                .get(start..end)
                .ok_or_else(|| MetagraphError::Malformed("account column is truncated".into()))?;
            return bytes
                .try_into()
                .map(AccountId)
                .map(Some)
                .map_err(|_| MetagraphError::Malformed("account column is not 32 bytes".into()));
        }
        let value = self.value(name, uid);
        if matches!(value, Value::Null) {
            Ok(None)
        } else {
            account(&value).map(Some)
        }
    }
    #[allow(deprecated)]
    fn owned_neuron(&self, uid: usize) -> Result<MetagraphNeuron> {
        let value = |name| self.value(name, uid);
        let balance = |name, netuid| -> Result<DynBalance> {
            let balance = DynBalance::from_rao(integer(&value(name))?, netuid);
            Ok(
                if netuid != NetUid::ROOT && !self.header.symbol.is_empty() {
                    balance.with_symbol(self.header.symbol.clone())
                } else {
                    balance
                },
            )
        };
        let has_collateral = !matches!(field(&self.fields, "collateral_locked"), Value::Null)
            || matches!(&self.columns, Columns::Borrowed(columns) if columns.compact.contains_key("collateral_locked"));
        let collateral = |name| {
            if has_collateral {
                balance(name, self.header.netuid).map(Some)
            } else {
                Ok(None)
            }
        };
        Ok(MetagraphNeuron {
            uid: Uid(u16::try_from(uid)
                .map_err(|_| MetagraphError::Malformed("UID exceeds u16".into()))?),
            hotkey: self
                .key_at("hotkeys", uid)?
                .ok_or_else(|| MetagraphError::Malformed("hotkey is absent".into()))?,
            coldkey: self.key_at("coldkeys", uid)?,
            active: truth(&value("active")),
            validator_permit: truth(&value("validator_permit")),
            last_update: integer(&value("last_update"))?,
            block_at_registration: integer(&value("block_at_registration"))?,
            rank: score(&value("rank"))?,
            trust: score(&value("trust"))?,
            consensus: score(&value("consensus"))?,
            incentive: score(&value("incentives"))?,
            dividends: score(&value("dividends"))?,
            pruning_score: score(&value("pruning_score"))?,
            emission: balance("emission", self.header.netuid)?,
            alpha_stake: balance("alpha_stake", self.header.netuid)?,
            tao_stake: balance("tao_stake", NetUid::ROOT)?,
            total_stake: StakeWeight(integer(&value("total_stake"))?),
            total_stake_display: balance("total_stake", self.header.netuid)?,
            axon: endpoint(&value("axons"))?,
            identity: record_json(&value("identities")),
            commitment: self
                .commitments
                .as_ref()
                .and_then(|entries| entries.get(&Uid(u16::try_from(uid).ok()?)))
                .cloned(),
            collateral_locked: collateral("collateral_locked")?,
            collateral_min: collateral("collateral_min")?,
            collateral_earned: collateral("collateral_earned")?,
        })
    }
}

/// A borrowed neuron row with lazy projection of its columns.
#[derive(Clone, Copy, Debug)]
pub struct NeuronRef<'a> {
    graph: &'a Metagraph,
    /// This row's UID.
    pub uid: Uid,
}
impl NeuronRef<'_> {
    pub(crate) fn axon_value(&self) -> Value {
        self.graph.value("axons", usize::from(self.uid.0))
    }
    /// Borrow the neuron's account ID.
    pub fn hotkey(&self) -> Option<&AccountId> {
        self.graph.hotkey(self.uid)
    }
    /// Whether this neuron holds a validator permit.
    pub fn validator_permit(&self) -> bool {
        truth(
            &self
                .graph
                .value("validator_permit", usize::from(self.uid.0)),
        )
    }
    /// Whether this neuron is active, without materializing the row.
    pub fn active(&self) -> bool {
        truth(&self.graph.value("active", usize::from(self.uid.0)))
    }
    /// Alpha-equivalent stake weight for admission and scoring comparisons.
    pub fn total_stake(&self) -> Result<StakeWeight> {
        integer(&self.graph.value("total_stake", usize::from(self.uid.0))).map(StakeWeight)
    }
    /// Materialize this row without decoding the response again.
    pub fn to_owned(&self) -> Result<MetagraphNeuron> {
        self.graph.owned_neuron(usize::from(self.uid.0))
    }
}

impl serde::Serialize for Metagraph {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.to_json()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl serde::Serialize for MetagraphNeuron {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

/// One neuron's complete Python-compatible metagraph row.
#[derive(Clone, Debug, PartialEq)]
pub struct MetagraphNeuron {
    /// Registered UID.
    pub uid: Uid,
    /// Registered hotkey.
    pub hotkey: AccountId,
    /// Registered coldkey; absent when an old/short column is padded.
    pub coldkey: Option<AccountId>,
    /// Active flag.
    pub active: bool,
    /// Validator permit.
    pub validator_permit: bool,
    /// Last weights update block.
    pub last_update: u64,
    /// Registration block.
    pub block_at_registration: u64,
    /// Legacy rank normalized by 65535.
    #[deprecated(note = "Runtime retains this dead field for compatibility")]
    pub rank: f64,
    /// Legacy trust normalized by 65535.
    #[deprecated(note = "Runtime retains this dead field for compatibility")]
    pub trust: f64,
    /// Consensus normalized by 65535.
    pub consensus: f64,
    /// Incentive normalized by 65535.
    pub incentive: f64,
    /// Dividends normalized by 65535.
    pub dividends: f64,
    /// Legacy pruning score normalized by 65535.
    #[deprecated(note = "Runtime retains this dead field for compatibility")]
    pub pruning_score: f64,
    /// Alpha emission in the subnet's units.
    pub emission: DynBalance,
    /// Alpha stake.
    pub alpha_stake: DynBalance,
    /// TAO stake in root units.
    pub tao_stake: DynBalance,
    /// Alpha-equivalent stake weight; this is not an amount of TAO.
    pub total_stake: StakeWeight,
    total_stake_display: DynBalance,
    /// Served IP endpoint, including IPv6 brackets.
    pub axon: Option<String>,
    /// Coldkey identity record.
    pub identity: Option<Json>,
    /// Registered commitment, when commitments were included in this snapshot.
    pub commitment: Option<NeuronCommitment>,
    /// Locked alpha collateral; absent before the chain added these columns.
    pub collateral_locked: Option<DynBalance>,
    /// Miner collateral floor.
    pub collateral_min: Option<DynBalance>,
    /// Lifetime collateral emission earned.
    pub collateral_earned: Option<DynBalance>,
}
impl MetagraphNeuron {
    /// Python dataclass field names and balance display strings.
    #[allow(deprecated)]
    pub fn to_json(&self) -> Json {
        json!({"uid":self.uid.0,"hotkey":self.hotkey.to_string(),"coldkey":self.coldkey.map_or_else(|| "None".into(), |key| key.to_string()),
            "active":self.active,"validator_permit":self.validator_permit,"last_update":self.last_update,"block_at_registration":self.block_at_registration,
            "rank":self.rank,"trust":self.trust,"consensus":self.consensus,"incentive":self.incentive,"dividends":self.dividends,"pruning_score":self.pruning_score,
            "emission":self.emission.to_string(),"alpha_stake":self.alpha_stake.to_string(),"tao_stake":self.tao_stake.to_string(),"total_stake":self.total_stake_display.to_string(),
            "axon":self.axon,"identity":self.identity,"commitment":self.commitment,
            "collateral_locked":self.collateral_locked.as_ref().map(ToString::to_string),"collateral_min":self.collateral_min.as_ref().map(ToString::to_string),"collateral_earned":self.collateral_earned.as_ref().map(ToString::to_string)})
    }
}

fn selective_defaults(
    mut graph: Value,
    netuid: NetUid,
    mechid: MechId,
    at: SnapshotInfo,
) -> Result<Value> {
    let Value::Dict(fields) = &mut graph else {
        return Err(MetagraphError::Malformed(
            "selective metagraph is not a record".into(),
        ));
    };
    let packed = rtao_primitives::NetUidStorageIndex::new(netuid, mechid)
        .map_err(|error| MetagraphError::Malformed(error.to_string()))?;
    let defaults = [
        ("netuid", Value::Int(i128::from(packed.0))),
        ("block", Value::Int(i128::from(at.number))),
        ("tempo", Value::Int(0)),
        ("last_step", Value::Int(0)),
        ("blocks_since_last_step", Value::Int(0)),
        ("network_registered_at", Value::Int(0)),
        ("owner_hotkey", Value::Bytes(vec![0; 32])),
        ("owner_coldkey", Value::Bytes(vec![0; 32])),
    ];
    for (name, default) in defaults {
        match fields.iter_mut().find(|(key, _)| key == &Value::str(name)) {
            Some((_, value)) if matches!(value, Value::Null) => *value = default,
            None => fields.push((Value::str(name), default)),
            _ => {}
        }
    }
    Ok(graph)
}

fn metagraph_payload_type(runtime: &RuntimeHandle, method: &str) -> Option<u32> {
    let method = runtime.runtime_api("SubnetInfoRuntimeApi", method)?;
    let option = runtime.core().types.resolve(method.output)?;
    if option.path.segments.as_slice() != ["Option"] {
        return None;
    }
    let scale_info::TypeDef::Variant(variants) = &option.type_def else {
        return None;
    };
    let some = variants
        .variants
        .iter()
        .find(|variant| variant.name == "Some" && variant.fields.len() == 1)?;
    Some(some.fields.first()?.ty.id)
}

fn check_moving_price_type(runtime: &RuntimeHandle, method: &str) -> Result<()> {
    let unsupported =
        || MetagraphError::Shape("moving_price semantic type is not signed I96F32".into());
    let payload = metagraph_payload_type(runtime, method).ok_or_else(unsupported)?;
    let record = runtime
        .core()
        .types
        .resolve(payload)
        .ok_or_else(unsupported)?;
    let scale_info::TypeDef::Composite(fields) = &record.type_def else {
        return Err(unsupported());
    };
    let moving_price = fields
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some("moving_price"))
        .ok_or_else(unsupported)?;
    let fixed = runtime
        .core()
        .types
        .resolve(moving_price.ty.id)
        .ok_or_else(unsupported)?;
    let fixed_id = if fixed.path.segments.as_slice() == ["Option"] {
        let scale_info::TypeDef::Variant(variants) = &fixed.type_def else {
            return Err(unsupported());
        };
        variants
            .variants
            .iter()
            .find(|variant| variant.name == "Some" && variant.fields.len() == 1)
            .and_then(|variant| variant.fields.first())
            .ok_or_else(unsupported)?
            .ty
            .id
    } else {
        moving_price.ty.id
    };
    let semantics = rtao_runtime::ShapeHasher::without_outer_enums(&runtime.core().types)
        .fixed_semantics(fixed_id)
        .map_err(|error| MetagraphError::Shape(error.to_string()))?;
    if semantics != Some((true, 128, 32)) {
        return Err(unsupported());
    }
    Ok(())
}

fn typed_gate(runtime: &RuntimeHandle, method: &str) -> Result<bool> {
    let Some(payload) = metagraph_payload_type(runtime, method) else {
        return Ok(false);
    };
    for model in shapes::models() {
        if let SurfaceDescriptor::Model(model) = model
            && model.type_path.ends_with("::Metagraph")
        {
            return Ok(runtime
                .projection_shape(payload, &model.projection)
                .is_ok_and(|shape| shape == model.shape));
        }
    }
    Ok(false)
}
fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    if let Value::Dict(fields) = value
        && let Some((_, value)) = fields
            .iter()
            .find(|(key, _)| matches!(key, Value::Str(key) if key == name))
    {
        return value;
    }
    &Value::Null
}
fn list(value: &Value) -> &[Value] {
    match value {
        Value::List(items) | Value::Tuple(items) => items,
        _ => &[],
    }
}
fn integer(value: &Value) -> Result<u64> {
    match value {
        Value::Null => Ok(0),
        Value::Bool(value) => Ok(u64::from(*value)),
        Value::Int(value) => u64::try_from(*value)
            .map_err(|_| MetagraphError::Malformed("integer is outside native u64".into())),
        Value::Uint(value) => u64::try_from(*value)
            .map_err(|_| MetagraphError::Malformed("integer is outside native u64".into())),
        _ => Err(MetagraphError::Malformed("expected native integer".into())),
    }
}
fn required_integer(fields: &Value, name: &str) -> Result<u64> {
    let value = field(fields, name);
    if matches!(value, Value::Null) {
        return Err(MetagraphError::Malformed(format!(
            "metagraph is missing {name}"
        )));
    }
    integer(value)
}
fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Int(value) => *value != 0,
        Value::Uint(value) => *value != 0,
        Value::Str(value) => !value.is_empty(),
        Value::Bytes(value) => !value.is_empty(),
        Value::List(value) | Value::Tuple(value) => !value.is_empty(),
        Value::Dict(value) => !value.is_empty(),
        Value::U256(value) => value.iter().any(|byte| *byte != 0),
    }
}
fn account(value: &Value) -> Result<AccountId> {
    match value {
        Value::Str(value) => value.parse().map_err(Into::into),
        Value::Bytes(bytes) => bytes
            .as_slice()
            .try_into()
            .map(AccountId)
            .map_err(|_| MetagraphError::Malformed("account is not 32 bytes".into())),
        _ => Err(MetagraphError::Malformed("expected account ID".into())),
    }
}
fn score(value: &Value) -> Result<f64> {
    Ok(integer(value)? as f64 / 65_535.0)
}
fn record_json(value: &Value) -> Option<Json> {
    matches!(value, Value::Dict(_)).then(|| to_corpus_json(value))
}
fn text(value: &Value) -> Result<String> {
    let bytes = match value {
        Value::Null => return Ok(String::new()),
        Value::Str(value) => {
            if let Some(hex) = value.strip_prefix("0x") {
                hex::decode(hex).map_err(|error| MetagraphError::Malformed(error.to_string()))?
            } else {
                return Ok(value.clone());
            }
        }
        Value::Bytes(bytes) => bytes.clone(),
        Value::List(values) | Value::Tuple(values) => values
            .iter()
            .map(|value| {
                u8::try_from(integer(value)?)
                    .map_err(|_| MetagraphError::Malformed("text byte exceeds u8".into()))
            })
            .collect::<Result<Vec<_>>>()?,
        other => return Ok(other.to_string()),
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn endpoint(value: &Value) -> Result<Option<String>> {
    if !matches!(value, Value::Dict(_)) {
        return Ok(None);
    }
    let ip = match field(value, "ip") {
        Value::Int(ip) => {
            u128::try_from(*ip).map_err(|_| MetagraphError::Malformed("negative axon IP".into()))?
        }
        Value::Uint(ip) => *ip,
        Value::Null => 0,
        _ => return Err(MetagraphError::Malformed("invalid axon IP".into())),
    };
    if ip == 0 {
        return Ok(None);
    }
    let port = u16::try_from(integer(field(value, "port"))?)
        .map_err(|_| MetagraphError::Malformed("axon port exceeds u16".into()))?;
    let ip_type = u8::try_from(integer(field(value, "ip_type"))?)
        .map_err(|_| MetagraphError::Malformed("axon IP type exceeds u8".into()))?;
    rtao_primitives::axon::axon_endpoint(ip, ip_type, port)
        .map(Some)
        .ok_or_else(|| MetagraphError::Malformed("IPv4 axon IP exceeds u32".into()))
}
