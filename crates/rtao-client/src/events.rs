//! Python-shaped event records and metadata-driven typed event decoding.

use std::sync::Arc;

use bytes::Bytes;
use rtao_core::{CoreError, codec::Value, runtime::TypeSpec, value_ext::ValueExt};
use rtao_primitives::H256;
use rtao_runtime::{EventDescriptor, RuntimeHandle};
use scale_decode::{
    DecodeAsFields, DecodeAsType, Field, IntoVisitor,
    visitor::{
        DecodeError, Visitor, decode_with_visitor,
        types::{Composite, Sequence, Variant},
    },
};
use scale_info::{PortableRegistry, TypeDef, form::PortableForm};

use crate::ClientError;

/// The execution phase in which an event was emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, DecodeAsType)]
pub enum Phase {
    /// An event associated with the given block extrinsic position.
    ApplyExtrinsic(u32),
    /// An event emitted while finalizing the block.
    Finalization,
    /// An event emitted while initializing the block.
    Initialization,
}

impl Phase {
    /// Return the extrinsic position, if this event belongs to an extrinsic.
    pub fn extrinsic_index(self) -> Option<u32> {
        match self {
            Self::ApplyExtrinsic(index) => Some(index),
            Self::Finalization | Self::Initialization => None,
        }
    }

    /// Return the phase name used by Python's decoded event records.
    pub fn name(self) -> &'static str {
        match self {
            Self::ApplyExtrinsic(_) => "ApplyExtrinsic",
            Self::Finalization => "Finalization",
            Self::Initialization => "Initialization",
        }
    }
}

/// An event record in the core's Python-compatible decoded shape.
///
/// [`Self::value`] retains the nested `event` dictionary and the flattened
/// fields exactly as the core decoded them.
#[derive(Clone, Debug, PartialEq)]
pub struct EventRecord {
    /// Python phase name.
    pub phase: String,
    /// Extrinsic position for `ApplyExtrinsic`, otherwise absent.
    pub extrinsic_idx: Option<u32>,
    /// Live pallet index, matching the Python top-level `event_index` field.
    pub event_index: u8,
    /// Pallet name used for event matching.
    pub module_id: String,
    /// Event variant name used for event matching.
    pub event_id: String,
    /// Core-decoded event fields, including their Python shape conventions.
    pub attributes: Value,
    /// Topics associated with this event.
    pub topics: Vec<H256>,
    value: Value,
}

impl EventRecord {
    /// Borrow the complete Python-shaped record without reconstructing it.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Match this record by live pallet and event names.
    pub fn matches(&self, pallet: &str, event: &str) -> bool {
        self.module_id == pallet && self.event_id == event
    }

    fn from_value(value: Value) -> Result<Self, ClientError> {
        let text = |name| {
            value
                .field(name)
                .and_then(ValueExt::as_str)
                .map(str::to_owned)
                .ok_or_else(|| schema(format!("event record has no string {name} field")))
        };
        let phase = text("phase")?;
        let module_id = text("module_id")?;
        let event_id = text("event_id")?;
        let extrinsic_idx = match value.field("extrinsic_idx") {
            Some(Value::Null) => None,
            Some(index) => Some(
                index
                    .as_u128()
                    .and_then(|index| u32::try_from(index).ok())
                    .ok_or_else(|| schema("invalid event extrinsic position"))?,
            ),
            None => return Err(schema("event record has no extrinsic_idx field")),
        };
        let event_index = value
            .field("event_index")
            .and_then(ValueExt::as_u128)
            .and_then(|index| u8::try_from(index).ok())
            .ok_or_else(|| schema("invalid event pallet index"))?;
        let attributes = value
            .field("attributes")
            .cloned()
            .ok_or_else(|| schema("event record has no attributes field"))?;
        let Some(Value::List(topics)) = value.field("topics") else {
            return Err(schema("event record topics are not a list"));
        };
        let topics = topics
            .iter()
            .map(|topic| {
                topic
                    .value_bytes()
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(H256)
                    .ok_or_else(|| schema("event topic is not a 32-byte hash"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            phase,
            extrinsic_idx,
            event_index,
            module_id,
            event_id,
            attributes,
            topics,
            value,
        })
    }
}

/// The metadata contract implemented by a typed event's field struct.
///
/// The struct should also derive [`DecodeAsType`], which supplies
/// [`DecodeAsFields`]. Matching uses names; descriptor indices never select
/// a record or determine the live field layout.
pub trait Event {
    /// Return the generated or explicitly declared event contract.
    fn descriptor() -> &'static EventDescriptor;
}

/// One event's identity and retained field bytes before typed decoding.
#[derive(Clone, Debug)]
pub struct RawEvent {
    phase: Phase,
    pallet: String,
    name: String,
    pallet_index: u8,
    event_index: u8,
    topics: Vec<H256>,
    field_bytes: Bytes,
    fields: Arc<[scale_info::Field<PortableForm>]>,
    runtime: RuntimeHandle,
}

impl RawEvent {
    /// Block execution phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Pallet name resolved from the live outer event enum.
    pub fn pallet(&self) -> &str {
        &self.pallet
    }

    /// Event variant name resolved from the live inner event enum.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Live outer enum index, available for diagnostics.
    pub fn pallet_index(&self) -> u8 {
        self.pallet_index
    }

    /// Live inner enum index, available for diagnostics.
    pub fn event_index(&self) -> u8 {
        self.event_index
    }

    /// Topics associated with this event.
    pub fn topics(&self) -> &[H256] {
        &self.topics
    }

    /// Borrow only this event's SCALE field bytes, excluding enum indices.
    pub fn field_bytes(&self) -> &[u8] {
        &self.field_bytes
    }

    /// Match the live pallet and event names.
    pub fn matches(&self, pallet: &str, event: &str) -> bool {
        self.pallet == pallet && self.name == event
    }

    /// Decode the requested event's fields after checking its live contract.
    ///
    /// A different pallet or event returns `Ok(None)`. A matching event whose
    /// fields have changed returns an error before decoding. Recorded indices
    /// may differ from the live metadata without preventing decoding.
    pub fn decode_as<E: Event + DecodeAsFields>(&self) -> Result<Option<E>, ClientError> {
        let descriptor = E::descriptor();
        if !self.matches(descriptor.pallet, descriptor.name) {
            return Ok(None);
        }
        if descriptor.fields.len() != self.fields.len()
            || descriptor
                .fields
                .iter()
                .zip(self.fields.iter())
                .any(|(expected, actual)| {
                    expected.name != actual.name.as_deref().unwrap_or_default()
                })
        {
            return Err(schema(format!(
                "event fields changed for {}.{}",
                self.pallet, self.name
            )));
        }
        let shape = self.runtime.fields_shape(&self.fields)?;
        if shape != descriptor.shape {
            return Err(schema(format!(
                "event field shape changed for {}.{}: expected {}, found {shape}",
                self.pallet, self.name, descriptor.shape
            )));
        }
        let mut fields = self
            .fields
            .iter()
            .map(|field| Field::new(field.ty.id, field.name.as_deref()));
        let mut input = self.field_bytes.as_ref();
        let event = E::decode_as_fields(&mut input, &mut fields, &self.runtime.core().types)
            .map_err(|error| schema(format!("{}.{}: {error}", self.pallet, self.name)))?;
        if !input.is_empty() {
            return Err(schema(format!(
                "{} trailing bytes in {}.{} event fields",
                input.len(),
                self.pallet,
                self.name
            )));
        }
        Ok(Some(event))
    }
}

/// Events from one `System.Events` value using its block's parent runtime.
#[derive(Clone, Debug)]
pub struct Events {
    records: Vec<EventRecord>,
    raw: Vec<RawEvent>,
}

impl Events {
    /// Decode Python-shaped records and retain each typed event's field range.
    ///
    /// Callers must select the parent runtime that executed the block. SCALE
    /// input is checked strictly through the core before the visitor walks its
    /// boundaries. Typed fields are decoded only by [`RawEvent::decode_as`].
    pub fn decode(runtime: RuntimeHandle, bytes: Bytes) -> Result<Self, ClientError> {
        let value_type = runtime
            .storage_entry("System", "Events")
            .ok_or_else(|| CoreError::NotInRuntime("System.Events".into()))?
            .value_type;
        let value = runtime
            .core()
            .decode_spec(&TypeSpec::Id(value_type), &bytes, true)
            .map_err(|error| decode_error(error.into()))?;
        let Value::List(values) = value else {
            return Err(schema("System.Events did not decode to a list"));
        };
        let records = values
            .into_iter()
            .map(EventRecord::from_value)
            .collect::<Result<Vec<_>, _>>()?;
        let mut input = bytes.as_ref();
        let borrowed =
            decode_with_visitor(&mut input, value_type, &runtime.core().types, EventsVisitor)
                .map_err(|error| schema(error.to_string()))?;
        if !input.is_empty() || borrowed.len() != records.len() {
            return Err(schema(
                "event visitor disagrees with the core record boundaries",
            ));
        }
        let raw = borrowed
            .into_iter()
            .zip(&records)
            .map(|(record, decoded)| {
                if record.phase.name() != decoded.phase
                    || record.phase.extrinsic_index() != decoded.extrinsic_idx
                    || record.event.pallet != decoded.module_id
                    || record.event.name != decoded.event_id
                    || record.event.pallet_index != decoded.event_index
                    || record.topics != decoded.topics
                {
                    return Err(schema(
                        "event visitor disagrees with the core record identity",
                    ));
                }
                let event = record.event;
                let ty = runtime
                    .core()
                    .resolve(event.type_id)
                    .map_err(CoreError::from)?;
                let TypeDef::Variant(variants) = &ty.type_def else {
                    return Err(schema("pallet event type is not an enum"));
                };
                let fields = variants
                    .variants
                    .iter()
                    .find(|variant| {
                        variant.index == event.event_index && variant.name == event.name
                    })
                    .ok_or_else(|| schema("event variant is absent from live metadata"))?
                    .fields
                    .clone()
                    .into();
                Ok(RawEvent {
                    phase: record.phase,
                    pallet: event.pallet,
                    name: event.name,
                    pallet_index: event.pallet_index,
                    event_index: event.event_index,
                    topics: record.topics,
                    field_bytes: bytes.slice_ref(event.fields),
                    fields,
                    runtime: runtime.clone(),
                })
            })
            .collect::<Result<_, ClientError>>()?;
        Ok(Self { records, raw })
    }

    /// Borrow records in their original block order.
    pub fn records(&self) -> &[EventRecord] {
        &self.records
    }

    /// Borrow raw records for optional typed field decoding.
    pub fn raw(&self) -> &[RawEvent] {
        &self.raw
    }

    /// Select Python-shaped records for one extrinsic position.
    pub fn for_extrinsic(&self, index: u32) -> impl Iterator<Item = &EventRecord> {
        self.records
            .iter()
            .filter(move |record| record.extrinsic_idx == Some(index))
    }

    /// Decode only records matching the typed event's pallet and name.
    pub fn find<E: Event + DecodeAsFields>(
        &self,
    ) -> impl Iterator<Item = Result<E, ClientError>> + '_ {
        self.raw
            .iter()
            .filter_map(|event| event.decode_as::<E>().transpose())
    }
}

fn decode_error(source: CoreError) -> ClientError {
    ClientError::Decode {
        what: "System.Events".into(),
        source,
    }
}

fn schema(message: impl Into<String>) -> ClientError {
    decode_error(CoreError::Codec(message.into()))
}

#[derive(Debug)]
enum WalkError {
    Decode(DecodeError),
    Schema(&'static str),
}

impl From<DecodeError> for WalkError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl std::fmt::Display for WalkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => error.fmt(formatter),
            Self::Schema(message) => formatter.write_str(message),
        }
    }
}

struct BorrowedEvent<'scale> {
    pallet: String,
    name: String,
    pallet_index: u8,
    event_index: u8,
    type_id: u32,
    fields: &'scale [u8],
}

struct BorrowedRecord<'scale> {
    phase: Phase,
    event: BorrowedEvent<'scale>,
    topics: Vec<H256>,
}

struct EventsVisitor;

impl Visitor for EventsVisitor {
    type Value<'scale, 'resolver> = Vec<BorrowedRecord<'scale>>;
    type Error = WalkError;
    type TypeResolver = PortableRegistry;

    fn visit_sequence<'scale, 'resolver>(
        self,
        sequence: &mut Sequence<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, WalkError> {
        let mut records = Vec::new();
        while let Some(record) = sequence.decode_item(RecordVisitor) {
            records.push(record?);
        }
        Ok(records)
    }
}

struct RecordVisitor;

impl Visitor for RecordVisitor {
    type Value<'scale, 'resolver> = BorrowedRecord<'scale>;
    type Error = WalkError;
    type TypeResolver = PortableRegistry;

    fn visit_composite<'scale, 'resolver>(
        self,
        record: &mut Composite<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, WalkError> {
        let mut phase = None;
        let mut event = None;
        let mut topics = None;
        while record.remaining() > 0 {
            match record.peek_name() {
                Some("phase") if phase.is_none() => {
                    phase = Some(
                        record
                            .decode_item(Phase::into_visitor::<PortableRegistry>())
                            .ok_or(WalkError::Schema("EventRecord has no phase"))?
                            .map_err(|_| WalkError::Schema("invalid event phase"))?,
                    );
                }
                Some("event") if event.is_none() => {
                    event = Some(
                        record
                            .decode_item(OuterEventVisitor)
                            .ok_or(WalkError::Schema("EventRecord has no event"))??,
                    );
                }
                Some("topics") if topics.is_none() => {
                    topics = Some(
                        record
                            .decode_item(Vec::<H256>::into_visitor::<PortableRegistry>())
                            .ok_or(WalkError::Schema("EventRecord has no topics"))?
                            .map_err(|_| WalkError::Schema("invalid event topics"))?,
                    );
                }
                _ => {
                    return Err(WalkError::Schema(
                        "unexpected or duplicated EventRecord field",
                    ));
                }
            }
        }
        Ok(BorrowedRecord {
            phase: phase.ok_or(WalkError::Schema("EventRecord has no phase"))?,
            event: event.ok_or(WalkError::Schema("EventRecord has no event"))?,
            topics: topics.ok_or(WalkError::Schema("EventRecord has no topics"))?,
        })
    }
}

struct OuterEventVisitor;

impl Visitor for OuterEventVisitor {
    type Value<'scale, 'resolver> = BorrowedEvent<'scale>;
    type Error = WalkError;
    type TypeResolver = PortableRegistry;

    fn visit_variant<'scale, 'resolver>(
        self,
        outer: &mut Variant<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, WalkError> {
        let pallet = outer.name().to_owned();
        let pallet_index = outer.index();
        if outer.fields().remaining() != 1 {
            return Err(WalkError::Schema(
                "outer event variant must have exactly one field",
            ));
        }
        let mut event = outer
            .fields()
            .decode_item(InnerEventVisitor)
            .ok_or(WalkError::Schema("outer event variant has no payload"))??;
        event.pallet = pallet;
        event.pallet_index = pallet_index;
        Ok(event)
    }
}

struct InnerEventVisitor;

impl Visitor for InnerEventVisitor {
    type Value<'scale, 'resolver> = BorrowedEvent<'scale>;
    type Error = WalkError;
    type TypeResolver = PortableRegistry;

    fn visit_variant<'scale, 'resolver>(
        self,
        inner: &mut Variant<'scale, 'resolver, PortableRegistry>,
        type_id: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, WalkError> {
        let name = inner.name().to_owned();
        let event_index = inner.index();
        let fields = inner.fields();
        let start = fields.bytes_from_start();
        fields.skip_decoding()?;
        let length = start
            .len()
            .checked_sub(fields.bytes_from_undecoded().len())
            .ok_or(WalkError::Schema("invalid event field boundary"))?;
        let fields = start
            .get(..length)
            .ok_or(WalkError::Schema("invalid event field range"))?;
        Ok(BorrowedEvent {
            pallet: String::new(),
            name,
            pallet_index: 0,
            event_index,
            type_id,
            fields,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
    use parity_scale_codec::{Decode, Encode};
    use rtao_core::codec::value::to_corpus_json;
    use rtao_primitives::TaoBalance;
    use rtao_runtime::events::balances::ISSUED;
    use scale_info::TypeDefPrimitive;
    use serde_json::Value as Json;

    #[derive(Debug, PartialEq, DecodeAsType)]
    struct Issued {
        amount: TaoBalance,
    }

    impl Event for Issued {
        fn descriptor() -> &'static EventDescriptor {
            &ISSUED
        }
    }

    #[derive(Debug, DecodeAsType)]
    struct CodeUpdated;

    impl Event for CodeUpdated {
        fn descriptor() -> &'static EventDescriptor {
            &rtao_runtime::events::system::CODE_UPDATED
        }
    }

    fn fixture() -> Json {
        rtao_primitives::json::from_str(include_str!("../../../vendor/upstream/golden.json"))
            .unwrap()
    }

    fn runtime() -> RuntimeHandle {
        let fixture = fixture();
        let metadata = rtao_core::ops::metadata::decode_v15(&fixture["metadata"]["v15_hex"])
            .unwrap()
            .unwrap();
        RuntimeHandle::parse(&metadata, 461, 1, 42).unwrap()
    }

    fn fixture_events() -> Events {
        let fixture = fixture();
        let bytes = hex::decode(
            fixture["events"]["raw_hex"]
                .as_str()
                .unwrap()
                .strip_prefix("0x")
                .unwrap(),
        )
        .unwrap();
        Events::decode(runtime(), bytes.into()).unwrap()
    }

    fn issued_bytes(pallet: u8, event: u8, amount: impl Encode, topics: Vec<[u8; 32]>) -> Bytes {
        let mut bytes = parity_scale_codec::Compact(1_u32).encode();
        bytes.extend([2, pallet, event]); // Initialization, outer enum, inner enum.
        bytes.extend(amount.encode());
        bytes.extend(topics.encode());
        bytes.into()
    }

    #[test]
    fn python_records_match_all_upstream_event_golden_values() {
        let events = fixture_events();
        let values: Vec<_> = events
            .records()
            .iter()
            .map(|record| to_corpus_json(record.value()))
            .collect();
        assert_eq!(Json::Array(values), fixture()["events"]["decoded"]);
        assert_eq!(events.records().len(), events.raw().len());
        assert!(
            events
                .records()
                .iter()
                .any(|record| record.extrinsic_idx.is_none())
        );
        assert!(
            events
                .for_extrinsic(2)
                .all(|record| record.extrinsic_idx == Some(2))
        );
        assert!(events.for_extrinsic(u32::MAX).next().is_none());
    }

    #[test]
    fn degraded_v14_reads_keep_the_same_golden_event_shapes() {
        let fixture = fixture();
        let metadata =
            rtao_core::ops::metadata::decode_legacy(&fixture["metadata"]["v14_hex"]).unwrap();
        let runtime = RuntimeHandle::parse(&metadata, 461, 1, 42).unwrap();
        assert_eq!(
            runtime.metadata_level(),
            rtao_runtime::MetadataLevel::V14Degraded
        );
        assert!(runtime.signing_status().is_err());
        let bytes = hex::decode(
            fixture["events"]["raw_hex"]
                .as_str()
                .unwrap()
                .strip_prefix("0x")
                .unwrap(),
        )
        .unwrap();
        let events = Events::decode(runtime, bytes.into()).unwrap();
        let values = events
            .records()
            .iter()
            .map(|record| to_corpus_json(record.value()))
            .collect();
        assert_eq!(Json::Array(values), fixture["events"]["decoded"]);
        assert_eq!(
            events.find::<Issued>().next().unwrap().unwrap().amount,
            TaoBalance(1_000_000_000)
        );
    }

    #[test]
    fn typed_events_decode_only_matching_live_names() {
        let events = fixture_events();
        let issued = events
            .find::<Issued>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            issued,
            [Issued {
                amount: TaoBalance(1_000_000_000)
            }]
        );
        let raw = events
            .raw()
            .iter()
            .find(|event| event.matches("Balances", "Issued"))
            .unwrap();
        assert_eq!(raw.phase, Phase::Initialization);
        assert_eq!(raw.field_bytes(), &1_000_000_000_u64.to_le_bytes());
        assert!(raw.decode_as::<CodeUpdated>().unwrap().is_none());
        assert_eq!(events.find::<CodeUpdated>().count(), 0);
    }

    #[test]
    fn topics_and_field_ranges_are_retained_without_enum_indices() {
        let topics = vec![[11; 32], [29; 32]];
        let bytes = issued_bytes(
            ISSUED.pallet_index,
            ISSUED.event_index,
            17_u64,
            topics.clone(),
        );
        let events = Events::decode(runtime(), bytes.clone()).unwrap();
        let raw = &events.raw()[0];
        assert_eq!(raw.topics, topics.into_iter().map(H256).collect::<Vec<_>>());
        assert_eq!(raw.topics, events.records()[0].topics);
        assert_eq!(
            raw.decode_as::<Issued>().unwrap().unwrap().amount,
            TaoBalance(17)
        );
        // Both Bytes values retain the same allocation. Only the event fields
        // occupy this range; topics and enum discriminants remain outside it.
        assert_eq!(raw.field_bytes().as_ptr(), bytes.get(4..).unwrap().as_ptr());
        assert_eq!(raw.field_bytes().len(), 8);
    }

    #[test]
    fn malformed_events_fail_before_any_record_is_returned() {
        let runtime = runtime();
        let complete = issued_bytes(5, 15, 17_u64, Vec::new());
        for end in 0..complete.len() {
            assert!(
                Events::decode(runtime.clone(), complete.slice(..end)).is_err(),
                "prefix {end}"
            );
        }
        let mut trailing = complete.to_vec();
        trailing.push(0);
        assert!(Events::decode(runtime.clone(), trailing.into()).is_err());
        assert!(Events::decode(runtime, issued_bytes(255, 15, 17_u64, Vec::new())).is_err());
    }

    #[test]
    fn empty_event_lists_have_both_empty_views() {
        let events = Events::decode(runtime(), Bytes::from_static(&[0])).unwrap();
        assert!(events.records().is_empty());
        assert!(events.raw().is_empty());
        assert_eq!(events.find::<Issued>().count(), 0);
    }

    fn mutate_runtime(
        change: impl FnOnce(&mut frame_metadata::v15::RuntimeMetadataV15),
    ) -> RuntimeHandle {
        let runtime = runtime();
        let mut metadata =
            RuntimeMetadataPrefixed::decode(&mut runtime.core().metadata_bytes.as_slice()).unwrap();
        let RuntimeMetadata::V15(v15) = &mut metadata.1 else {
            panic!("V15 fixture required");
        };
        change(v15);
        RuntimeHandle::parse(&metadata.encode(), 462, 1, 42).unwrap()
    }

    #[test]
    fn pallet_and_event_index_drift_still_matches_the_same_named_contract() {
        let runtime = mutate_runtime(|metadata| {
            let pallet = metadata
                .pallets
                .iter_mut()
                .find(|pallet| pallet.name == "Balances")
                .unwrap();
            pallet.index = 231;
            let inner_id = pallet.event.as_ref().unwrap().ty.id;
            let outer_id = metadata.outer_enums.event_enum_ty.id;
            for portable in &mut metadata.types.types {
                let TypeDef::Variant(variants) = &mut portable.ty.type_def else {
                    continue;
                };
                if portable.id == outer_id {
                    variants
                        .variants
                        .iter_mut()
                        .find(|variant| variant.name == "Balances")
                        .unwrap()
                        .index = 231;
                }
                if portable.id == inner_id {
                    variants
                        .variants
                        .iter_mut()
                        .find(|variant| variant.name == "Issued")
                        .unwrap()
                        .index = 229;
                }
            }
        });
        let events = Events::decode(runtime, issued_bytes(231, 229, 19_u64, Vec::new())).unwrap();
        assert_eq!(events.raw()[0].pallet_index, 231);
        assert_eq!(events.raw()[0].event_index, 229);
        assert_eq!(
            events.raw()[0]
                .decode_as::<Issued>()
                .unwrap()
                .unwrap()
                .amount,
            TaoBalance(19)
        );
        assert_eq!(events.records()[0].event_index, 231);
        assert_eq!(
            events.records()[0]
                .value()
                .field("event")
                .unwrap()
                .field("event_index"),
            Some(&Value::str("e7e5"))
        );
    }

    #[test]
    fn typed_decode_rejects_changed_field_shape_but_dynamic_records_remain_readable() {
        let runtime = mutate_runtime(|metadata| {
            let u32_id = metadata
                .types
                .types
                .iter()
                .find(|portable| {
                    matches!(
                        portable.ty.type_def,
                        TypeDef::Primitive(TypeDefPrimitive::U32)
                    )
                })
                .unwrap()
                .id;
            let pallet = metadata
                .pallets
                .iter()
                .find(|pallet| pallet.name == "Balances")
                .unwrap();
            let inner_id = pallet.event.as_ref().unwrap().ty.id;
            let portable = metadata
                .types
                .types
                .iter_mut()
                .find(|portable| portable.id == inner_id)
                .unwrap();
            let TypeDef::Variant(variants) = &mut portable.ty.type_def else {
                panic!("event enum required");
            };
            let issued = variants
                .variants
                .iter_mut()
                .find(|variant| variant.name == "Issued")
                .unwrap();
            issued.fields[0].ty.id = u32_id;
        });
        let events = Events::decode(runtime, issued_bytes(5, 15, 23_u32, Vec::new())).unwrap();
        assert_eq!(
            events.records()[0].attributes.field("amount"),
            Some(&Value::Int(23))
        );
        assert!(
            events.raw()[0]
                .decode_as::<Issued>()
                .unwrap_err()
                .to_string()
                .contains("event field shape changed")
        );
    }
}
