//! Verification of generated descriptors against live runtime metadata.

use std::collections::BTreeSet;

use rtao_core::CoreError;
use rtao_core::runtime::Runtime;
use scale_info::{Field, TypeDef, Variant, form::PortableForm};

use crate::descriptor::{
    ApiSurface, Call, CallDescriptor, ConstantSurface, EventDescriptor, FieldDescriptor,
    ModelSurface, StorageSurface, SurfaceDescriptor, shape_parts,
};
use crate::handle::{CallBytes, ComposeError, MetadataLevel, RuntimeHandle};

/// Consequence of a descriptor difference from the compiled surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SurfaceClass {
    /// A signing contract changed, or a required chain item was removed.
    Breaking,
    /// A read contract, API version, or missing-value behaviour changed.
    Behavioural,
    /// A previously unknown named item was added.
    Additive,
}

/// Kind of metadata item described by a surface change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SurfaceKind {
    /// Runtime metadata capabilities.
    Metadata,
    /// Extrinsic encoding version.
    Extrinsic,
    /// A pallet name or outer index.
    Pallet,
    /// A callable variant.
    Call,
    /// A storage entry.
    Storage,
    /// A runtime constant.
    Constant,
    /// A runtime API method.
    RuntimeApi,
    /// An event variant.
    Event,
    /// A hand-written model's declared field projection.
    Model,
    /// An extrinsic signed extension.
    Extension,
    /// A dispatch module error.
    ModuleError,
}

/// One difference between a compiled descriptor and live metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceChange {
    /// Stable descriptor identity, such as `call:Balances.transfer_keep_alive`.
    pub descriptor: String,
    /// Metadata category of this item.
    pub kind: SurfaceKind,
    /// Consequence for compatibility and drift reporting.
    pub class: SurfaceClass,
    /// Human-readable cause of the difference.
    pub detail: String,
}

/// Verification results for the used typed chain surface.
///
/// Descriptor failures identify the calls that refuse composition and reads
/// that need a dynamic fallback. A runtime-wide signing guard can refuse every call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SurfaceReport {
    /// Number of compiled descriptors that were checked.
    pub checked: usize,
    /// Differences in deterministic descriptor order.
    pub changes: Vec<SurfaceChange>,
    /// API descriptors whose live API version was not supplied by the client.
    ///
    /// V15 metadata alone cannot verify API versions. Their signatures are
    /// still checked, but these entries must not be described as fully verified.
    pub unverified_api_versions: BTreeSet<String>,
    checked_descriptors: BTreeSet<String>,
    incompatible_shapes: BTreeSet<String>,
}

impl SurfaceReport {
    /// Count distinct descriptors with a breaking difference.
    pub fn breaking_items(&self) -> usize {
        self.count(SurfaceClass::Breaking)
    }

    /// Count distinct descriptors with a behavioural difference.
    pub fn behavioural_items(&self) -> usize {
        self.count(SurfaceClass::Behavioural)
    }

    /// Count distinct descriptors added to the live surface.
    pub fn additive_items(&self) -> usize {
        self.count(SurfaceClass::Additive)
    }

    /// Whether every checked typed descriptor retains its encoding shape.
    pub fn shapes_match(&self) -> bool {
        self.incompatible_shapes.is_empty()
    }

    /// Whether one checked descriptor retains its encoding shape.
    ///
    /// Callers use the stable identity carried by [`SurfaceChange::descriptor`].
    /// Behavioural changes and moved indices do not imply a shape mismatch.
    /// A descriptor omitted from the checked set always returns `false`.
    pub fn shape_matches(&self, descriptor: &str) -> bool {
        self.checked_descriptors.contains(descriptor)
            && !self.incompatible_shapes.contains(descriptor)
    }

    /// Whether the used surface has no breaking drift.
    pub fn is_compatible(&self) -> bool {
        self.breaking_items() == 0
    }

    fn count(&self, class: SurfaceClass) -> usize {
        self.changes
            .iter()
            .filter(|change| change.class == class)
            .map(|change| change.descriptor.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    }

    fn push(
        &mut self,
        id: &str,
        kind: SurfaceKind,
        class: SurfaceClass,
        detail: impl Into<String>,
    ) {
        self.changes.push(SurfaceChange {
            descriptor: id.into(),
            kind,
            class,
            detail: detail.into(),
        });
    }

    fn incompatible(&mut self, id: &str, kind: SurfaceKind, detail: impl Into<String>) {
        self.incompatible_class(id, kind, SurfaceClass::Breaking, detail);
    }

    fn incompatible_class(
        &mut self,
        id: &str,
        kind: SurfaceKind,
        class: SurfaceClass,
        detail: impl Into<String>,
    ) {
        self.incompatible_shapes.insert(id.into());
        self.push(id, kind, class, detail);
    }

    fn read_incompatible(&mut self, id: &str, kind: SurfaceKind, detail: impl Into<String>) {
        self.incompatible_class(id, kind, SurfaceClass::Behavioural, detail);
    }

    fn finish(&mut self) {
        self.changes.sort_by(|left, right| {
            (&left.descriptor, left.kind, left.class, &left.detail).cmp(&(
                &right.descriptor,
                right.kind,
                right.class,
                &right.detail,
            ))
        });
    }
}

fn id(kind: &str, owner: &str, name: &str) -> String {
    format!("{kind}:{owner}.{name}")
}

fn variants<'a>(
    runtime: &'a Runtime,
    type_id: Option<u32>,
    kind: &str,
) -> Result<&'a [Variant<PortableForm>], String> {
    let type_id = type_id.ok_or_else(|| format!("{kind} enum is absent"))?;
    let ty = runtime
        .resolve(type_id)
        .map_err(|error| error.to_string())?;
    let TypeDef::Variant(variants) = &ty.type_def else {
        return Err(format!("{kind} type is not an enum"));
    };
    Ok(&variants.variants)
}

fn call_variant<'a>(
    runtime: &'a Runtime,
    descriptor: &CallDescriptor,
) -> Result<&'a Variant<PortableForm>, String> {
    let pallet = runtime
        .pallet(descriptor.pallet)
        .ok_or_else(|| format!("pallet {} is absent", descriptor.pallet))?;
    variants(runtime, pallet.calls_type, "call")?
        .iter()
        .find(|call| call.name == descriptor.name)
        .ok_or_else(|| format!("call {}.{} is absent", descriptor.pallet, descriptor.name))
}

fn verify_fields(
    expected: &[FieldDescriptor],
    actual: &[Field<PortableForm>],
) -> Result<(), String> {
    if expected.len() != actual.len() {
        return Err(format!(
            "field count changed from {} to {}",
            expected.len(),
            actual.len()
        ));
    }
    for (expected, actual) in expected.iter().zip(actual) {
        let name = actual.name.as_deref().unwrap_or_default();
        if expected.name != name {
            return Err(format!(
                "field name/order changed: expected {:?}, found {name:?}",
                expected.name
            ));
        }
    }
    Ok(())
}

fn verify_call_shape(runtime: &RuntimeHandle, descriptor: &CallDescriptor) -> Result<(), String> {
    let call = call_variant(runtime.core(), descriptor)?;
    verify_fields(descriptor.fields, &call.fields)?;
    let actual = runtime
        .fields_shape(&call.fields)
        .map_err(|error| error.to_string())?;
    if descriptor.shape != actual {
        return Err(format!(
            "argument shape changed from {} to {actual}",
            descriptor.shape
        ));
    }
    Ok(())
}

/// Check one call's live contract without rejecting unrelated drift.
///
/// A moved pallet or call index is breaking drift and requires regenerated
/// descriptors, even when its argument encoding is unchanged.
pub fn verify_call(runtime: &RuntimeHandle, descriptor: &CallDescriptor) -> Result<(), String> {
    verify_call_shape(runtime, descriptor)?;
    if runtime.pallet_index(descriptor.pallet) != Some(descriptor.pallet_index)
        || runtime.call_index(descriptor.pallet, descriptor.name) != Some(descriptor.call_index)
    {
        return Err(
            "call pallet/variant index changed; regenerated descriptors are required".into(),
        );
    }
    Ok(())
}

impl Call {
    /// Verify this call and encode its arguments through the upstream core.
    ///
    /// Nested calls must carry the same specification and transaction versions.
    /// The core resolves pallet and call indices from this runtime by name.
    pub fn compose(&self, runtime: &RuntimeHandle) -> Result<CallBytes, ComposeError> {
        if runtime.metadata_level() != MetadataLevel::V15 {
            return Err(ComposeError::MetadataDegraded);
        }
        runtime
            .signing_status()
            .map_err(ComposeError::SigningDisabled)?;
        for stamp in &self.nested_stamps {
            if *stamp != runtime.stamp() {
                return Err(ComposeError::VersionMismatch {
                    composed: *stamp,
                    runtime: runtime.stamp(),
                });
            }
        }
        verify_call(runtime, self.descriptor).map_err(|detail| ComposeError::SurfaceChanged {
            pallet: self.descriptor.pallet.into(),
            call: self.descriptor.name.into(),
            detail,
        })?;
        let bytes = runtime
            .core()
            .compose_call(self.descriptor.pallet, self.descriptor.name, &self.args)
            .map_err(CoreError::from)?;
        Ok(CallBytes::from_encoded(bytes, runtime))
    }
}

fn verify_storage(runtime: &RuntimeHandle, expected: StorageSurface, report: &mut SurfaceReport) {
    let id = id("storage", expected.pallet, expected.name);
    let Some(entry) = runtime.storage_entry(expected.pallet, expected.name) else {
        report.incompatible(&id, SurfaceKind::Storage, "storage entry is absent");
        return;
    };
    let mut types = entry.key_types.clone();
    types.push(entry.value_type);
    match runtime.tuple_shape(&types) {
        Ok(actual) if actual == expected.shape => {}
        Ok(actual) => report.read_incompatible(
            &id,
            SurfaceKind::Storage,
            format!(
                "key/value shape changed from {} to {actual}",
                expected.shape
            ),
        ),
        Err(error) => report.read_incompatible(&id, SurfaceKind::Storage, error.to_string()),
    }
    if entry.prefix != expected.prefix {
        report.push(
            &id,
            SurfaceKind::Storage,
            SurfaceClass::Behavioural,
            "storage prefix changed; keys use the live prefix",
        );
    }
    if entry
        .hashers
        .iter()
        .map(String::as_str)
        .ne(expected.hashers.iter().copied())
    {
        report.push(
            &id,
            SurfaceKind::Storage,
            SurfaceClass::Behavioural,
            "storage hashers changed; keys use the live hashers",
        );
    }
    if entry.modifier != expected.modifier {
        report.push(
            &id,
            SurfaceKind::Storage,
            SurfaceClass::Behavioural,
            "storage missing-value modifier changed",
        );
    }
    if entry.default_bytes != expected.default_bytes {
        report.push(
            &id,
            SurfaceKind::Storage,
            SurfaceClass::Behavioural,
            "storage default bytes changed",
        );
    }
}

fn verify_constant(runtime: &RuntimeHandle, expected: ConstantSurface, report: &mut SurfaceReport) {
    let id = id("constant", expected.pallet, expected.name);
    let Some(constant) = runtime.core().constant(expected.pallet, expected.name) else {
        report.read_incompatible(&id, SurfaceKind::Constant, "constant is absent");
        return;
    };
    match runtime.exact_shape(constant.ty) {
        Ok(actual) if actual == expected.shape => {}
        Ok(actual) => report.read_incompatible(
            &id,
            SurfaceKind::Constant,
            format!("constant shape changed from {} to {actual}", expected.shape),
        ),
        Err(error) => report.read_incompatible(&id, SurfaceKind::Constant, error.to_string()),
    }
    if !(expected.pallet == "System" && expected.name == "Version")
        && constant.value != expected.value
    {
        report.push(
            &id,
            SurfaceKind::Constant,
            SurfaceClass::Behavioural,
            "constant value changed",
        );
    }
}

fn verify_api(runtime: &RuntimeHandle, expected: ApiSurface, report: &mut SurfaceReport) {
    let id = id("api", expected.api, expected.name);
    match runtime.runtime_api_version(expected.api) {
        Some(version) if version != expected.version => report.push(
            &id,
            SurfaceKind::RuntimeApi,
            SurfaceClass::Behavioural,
            format!(
                "runtime API version changed from {} to {version}",
                expected.version
            ),
        ),
        Some(_) => {}
        None => {
            report.unverified_api_versions.insert(id.clone());
        }
    }
    if runtime.metadata_level() == MetadataLevel::V14Degraded {
        // A client's frozen legacy API table decides availability for this
        // level. V14 does not describe modern API signatures.
        report.read_incompatible(
            &id,
            SurfaceKind::RuntimeApi,
            "runtime API signatures require V15 metadata",
        );
        return;
    }
    let Some(method) = runtime.runtime_api(expected.api, expected.name) else {
        report.read_incompatible(&id, SurfaceKind::RuntimeApi, "runtime API method is absent");
        return;
    };
    if expected.inputs.len() != method.inputs.len()
        || expected
            .inputs
            .iter()
            .zip(&method.inputs)
            .any(|(expected, actual)| expected.name != actual.name)
    {
        report.read_incompatible(
            &id,
            SurfaceKind::RuntimeApi,
            "runtime API input names or order changed",
        );
    }
    let actual = (|| {
        let mut parts = Vec::with_capacity(method.inputs.len() + 1);
        for input in &method.inputs {
            parts.push((input.name.as_str(), runtime.exact_shape(input.ty)?));
        }
        let output = match expected.output_projection {
            Some(make) => runtime.projection_shape(method.output, &make())?,
            None => runtime.exact_shape(method.output)?,
        };
        parts.push(("$output", output));
        Ok::<_, CoreError>(shape_parts(&parts))
    })();
    match actual {
        Ok(actual) if actual == expected.shape => {}
        Ok(actual) => report.read_incompatible(
            &id,
            SurfaceKind::RuntimeApi,
            format!(
                "runtime API shape changed from {} to {actual}",
                expected.shape
            ),
        ),
        Err(error) => report.read_incompatible(&id, SurfaceKind::RuntimeApi, error.to_string()),
    }
}

fn verify_event(runtime: &RuntimeHandle, expected: &EventDescriptor, report: &mut SurfaceReport) {
    let id = id("event", expected.pallet, expected.name);
    let event = (|| {
        let pallet = runtime
            .core()
            .pallet(expected.pallet)
            .ok_or_else(|| "pallet is absent".to_string())?;
        let event = variants(runtime.core(), pallet.events_type, "event")?
            .iter()
            .find(|event| event.name == expected.name)
            .ok_or_else(|| "event is absent".to_string())?;
        verify_fields(expected.fields, &event.fields)?;
        let actual = runtime
            .fields_shape(&event.fields)
            .map_err(|error| error.to_string())?;
        if actual != expected.shape {
            return Err(format!(
                "event field shape changed from {} to {actual}",
                expected.shape
            ));
        }
        Ok((pallet.index, event.index))
    })();
    match event {
        Err(error) => report.read_incompatible(&id, SurfaceKind::Event, error),
        Ok((pallet, event)) if pallet != expected.pallet_index || event != expected.event_index => {
            report.push(
                &id,
                SurfaceKind::Event,
                SurfaceClass::Behavioural,
                "event pallet/variant index changed",
            );
        }
        Ok(_) => {}
    }
}

fn verify_model(runtime: &RuntimeHandle, expected: &ModelSurface, report: &mut SurfaceReport) {
    let id = format!("model:{}", expected.type_path);
    let mut matches = runtime
        .core()
        .types
        .types
        .iter()
        .filter(|ty| ty.ty.path.segments.join("::") == expected.type_path);
    let Some(ty) = matches.next() else {
        report.read_incompatible(&id, SurfaceKind::Model, "model type path is absent");
        return;
    };
    if matches.next().is_some() {
        report.read_incompatible(&id, SurfaceKind::Model, "model type path is ambiguous");
        return;
    }
    match runtime.projection_shape(ty.id, &expected.projection) {
        Ok(actual) if actual == expected.shape => {}
        Ok(actual) => report.read_incompatible(
            &id,
            SurfaceKind::Model,
            format!(
                "model projection shape changed from {} to {actual}",
                expected.shape
            ),
        ),
        Err(error) => report.read_incompatible(&id, SurfaceKind::Model, error.to_string()),
    }
}

/// Verify a supplied descriptor set against live metadata.
///
/// This supports tooling and tests in addition to the generated used surface.
/// The report does not prevent a call with an unchanged contract from encoding.
pub fn verify_surface_with(
    runtime: &RuntimeHandle,
    descriptors: &[SurfaceDescriptor],
) -> SurfaceReport {
    let mut report = SurfaceReport::default();
    for descriptor in descriptors {
        report.checked += 1;
        report.checked_descriptors.insert(match descriptor {
            SurfaceDescriptor::Call(expected) => id("call", expected.pallet, expected.name),
            SurfaceDescriptor::Storage(expected) => id("storage", expected.pallet, expected.name),
            SurfaceDescriptor::Constant(expected) => id("constant", expected.pallet, expected.name),
            SurfaceDescriptor::RuntimeApi(expected) => id("api", expected.api, expected.name),
            SurfaceDescriptor::Event(expected) => id("event", expected.pallet, expected.name),
            SurfaceDescriptor::Model(expected) => format!("model:{}", expected.type_path),
        });
        match descriptor {
            SurfaceDescriptor::Call(expected) => {
                let id = id("call", expected.pallet, expected.name);
                if let Err(error) = verify_call_shape(runtime, expected) {
                    report.incompatible(&id, SurfaceKind::Call, error);
                }
                if runtime
                    .pallet_index(expected.pallet)
                    .is_some_and(|index| index != expected.pallet_index)
                    || runtime
                        .call_index(expected.pallet, expected.name)
                        .is_some_and(|index| index != expected.call_index)
                {
                    report.push(
                        &id,
                        SurfaceKind::Call,
                        SurfaceClass::Breaking,
                        "call pallet/variant index changed; regenerated descriptors are required",
                    );
                }
            }
            SurfaceDescriptor::Storage(expected) => verify_storage(runtime, *expected, &mut report),
            SurfaceDescriptor::Constant(expected) => {
                verify_constant(runtime, *expected, &mut report)
            }
            SurfaceDescriptor::RuntimeApi(expected) => verify_api(runtime, *expected, &mut report),
            SurfaceDescriptor::Event(expected) => verify_event(runtime, expected, &mut report),
            SurfaceDescriptor::Model(expected) => verify_model(runtime, expected, &mut report),
        }
    }
    report.finish();
    report
}

/// Check all descriptors used by the compiled typed surface.
///
/// Clients run this at connect and after each runtime upgrade. The handle
/// caches the report when accessed through [`RuntimeHandle::surface_report`].
pub fn verify_surface(runtime: &RuntimeHandle) -> SurfaceReport {
    let mut descriptors = crate::generated::shapes::USED.to_vec();
    descriptors.extend(crate::generated::shapes::models());
    let mut report = verify_surface_with(runtime, &descriptors);
    verify_global_surface(runtime, &mut report);
    report.finish();
    report
}

fn verify_global_surface(runtime: &RuntimeHandle, report: &mut SurfaceReport) {
    if runtime.metadata_level() != MetadataLevel::V15 {
        report.push(
            "metadata:level",
            SurfaceKind::Metadata,
            SurfaceClass::Breaking,
            "V15 metadata is unavailable; signing is refused",
        );
    }
    if runtime.core().extrinsic.version != 4 {
        report.push(
            "extrinsic:version",
            SurfaceKind::Extrinsic,
            SurfaceClass::Breaking,
            format!(
                "core signing requires extrinsic version 4, found {}",
                runtime.core().extrinsic.version
            ),
        );
    }
    for expected in crate::generated::PALLETS {
        let id = format!("pallet:{}", expected.name);
        match runtime.pallet_index(expected.name) {
            None => report.push(
                &id,
                SurfaceKind::Pallet,
                SurfaceClass::Breaking,
                "pallet is absent",
            ),
            Some(index) if index != expected.index => report.push(
                &id,
                SurfaceKind::Pallet,
                SurfaceClass::Breaking,
                "pallet outer index changed",
            ),
            Some(_) => {}
        }
    }
    for pallet in &runtime.core().pallets {
        if !crate::generated::PALLETS
            .iter()
            .any(|expected| expected.name == pallet.name)
        {
            report.push(
                &format!("pallet:{}", pallet.name),
                SurfaceKind::Pallet,
                SurfaceClass::Additive,
                "pallet was added",
            );
        }
    }
    let expected = crate::generated::extensions::EXPECTED;
    let live = &runtime.core().extrinsic.signed_extensions;
    if live
        .iter()
        .map(|extension| extension.identifier.as_str())
        .ne(expected.iter().map(|extension| extension.identifier))
    {
        report.push(
            "extensions:order",
            SurfaceKind::Extension,
            SurfaceClass::Behavioural,
            "signed extension identifier list/order changed",
        );
    }
    for extension in expected {
        let id = format!("extension:{}", extension.identifier);
        let Some(actual) = live
            .iter()
            .find(|actual| actual.identifier == extension.identifier)
        else {
            report.push(
                &id,
                SurfaceKind::Extension,
                SurfaceClass::Breaking,
                "signed extension was removed",
            );
            continue;
        };
        let shapes = runtime.exact_shape(actual.ty).and_then(|shape| {
            runtime
                .exact_shape(actual.additional_signed)
                .map(|additional| (shape, additional))
        });
        match shapes {
            Ok((shape, additional))
                if shape == extension.shape && additional == extension.additional_signed_shape => {}
            Ok(_) => report.push(
                &id,
                SurfaceKind::Extension,
                SurfaceClass::Breaking,
                "signed extension payload shape changed",
            ),
            Err(error) => report.push(
                &id,
                SurfaceKind::Extension,
                SurfaceClass::Breaking,
                error.to_string(),
            ),
        }
    }
    for extension in live {
        if !expected
            .iter()
            .any(|expected| expected.identifier == extension.identifier)
        {
            let class = if runtime.signing_status().is_ok() {
                SurfaceClass::Additive
            } else {
                SurfaceClass::Breaking
            };
            report.push(
                &format!("extension:{}", extension.identifier),
                SurfaceKind::Extension,
                class,
                "signed extension was added; the core extension probe determines signing support",
            );
        }
    }
    verify_module_errors(runtime, report);
    report_additions(runtime, crate::generated::shapes::ALL, report);
}

fn verify_module_errors(runtime: &RuntimeHandle, report: &mut SurfaceReport) {
    let expected = crate::generated::errors::MODULE_ERRORS;
    for error in expected {
        let id = id("error", error.pallet, error.name);
        let actual = runtime.core().pallet(error.pallet).and_then(|pallet| {
            variants(runtime.core(), pallet.errors_type, "error")
                .ok()
                .and_then(|errors| {
                    errors
                        .iter()
                        .find(|actual| actual.name == error.name)
                        .map(|actual| (pallet.index, actual.index))
                })
        });
        match actual {
            None => report.push(
                &id,
                SurfaceKind::ModuleError,
                SurfaceClass::Breaking,
                "module error is absent",
            ),
            Some((pallet, index)) if pallet != error.pallet_index || index != error.error_index => {
                report.push(
                    &id,
                    SurfaceKind::ModuleError,
                    SurfaceClass::Breaking,
                    "module error pallet/variant index changed",
                );
            }
            Some(_) => {}
        }
    }
    for pallet in &runtime.core().pallets {
        if !expected.iter().any(|error| error.pallet == pallet.name) {
            continue;
        }
        if let Ok(errors) = variants(runtime.core(), pallet.errors_type, "error") {
            for error in errors {
                if !expected
                    .iter()
                    .any(|expected| expected.pallet == pallet.name && expected.name == error.name)
                {
                    report.push(
                        &id("error", &pallet.name, &error.name),
                        SurfaceKind::ModuleError,
                        SurfaceClass::Additive,
                        "module error was added",
                    );
                }
            }
        }
    }
}

fn report_additions(
    runtime: &RuntimeHandle,
    descriptors: &[SurfaceDescriptor],
    report: &mut SurfaceReport,
) {
    let mut expected_calls = BTreeSet::new();
    let mut expected_storage = BTreeSet::new();
    let mut expected_constants = BTreeSet::new();
    let mut expected_events = BTreeSet::new();
    let mut expected_apis = BTreeSet::new();
    for descriptor in descriptors {
        match descriptor {
            SurfaceDescriptor::Call(expected) => {
                expected_calls.insert((expected.pallet, expected.name));
            }
            SurfaceDescriptor::Storage(expected) => {
                expected_storage.insert((expected.pallet, expected.name));
            }
            SurfaceDescriptor::Constant(expected) => {
                expected_constants.insert((expected.pallet, expected.name));
            }
            SurfaceDescriptor::Event(expected) => {
                expected_events.insert((expected.pallet, expected.name));
            }
            SurfaceDescriptor::RuntimeApi(expected) => {
                expected_apis.insert((expected.api, expected.name));
            }
            SurfaceDescriptor::Model(_) => {}
        }
    }
    for pallet in &runtime.core().pallets {
        if expected_calls
            .iter()
            .any(|(owner, _)| *owner == pallet.name)
            && let Ok(calls) = variants(runtime.core(), pallet.calls_type, "call")
        {
            for call in calls {
                if !expected_calls.contains(&(pallet.name.as_str(), call.name.as_str())) {
                    report.push(
                        &id("call", &pallet.name, &call.name),
                        SurfaceKind::Call,
                        SurfaceClass::Additive,
                        "call was added",
                    );
                }
            }
        }
        if expected_storage
            .iter()
            .any(|(owner, _)| *owner == pallet.name)
        {
            for entry in &pallet.storage {
                if !entry.name.contains(':')
                    && !expected_storage.contains(&(pallet.name.as_str(), entry.name.as_str()))
                {
                    report.push(
                        &id("storage", &pallet.name, &entry.name),
                        SurfaceKind::Storage,
                        SurfaceClass::Additive,
                        "storage entry was added",
                    );
                }
            }
        }
        if expected_constants
            .iter()
            .any(|(owner, _)| *owner == pallet.name)
        {
            for constant in &pallet.constants {
                if pallet.name == "System" && constant.name == "Version" {
                    continue;
                }
                if !expected_constants.contains(&(pallet.name.as_str(), constant.name.as_str())) {
                    report.push(
                        &id("constant", &pallet.name, &constant.name),
                        SurfaceKind::Constant,
                        SurfaceClass::Additive,
                        "constant was added",
                    );
                }
            }
        }
        if expected_events
            .iter()
            .any(|(owner, _)| *owner == pallet.name)
            && let Ok(events) = variants(runtime.core(), pallet.events_type, "event")
        {
            for event in events {
                if !expected_events.contains(&(pallet.name.as_str(), event.name.as_str())) {
                    report.push(
                        &id("event", &pallet.name, &event.name),
                        SurfaceKind::Event,
                        SurfaceClass::Additive,
                        "event was added",
                    );
                }
            }
        }
    }
    for api in &runtime.core().apis {
        if expected_apis.iter().any(|(owner, _)| *owner == api.name) {
            for method in &api.methods {
                if !expected_apis.contains(&(api.name.as_str(), method.name.as_str())) {
                    report.push(
                        &id("api", &api.name, &method.name),
                        SurfaceKind::RuntimeApi,
                        SurfaceClass::Additive,
                        "runtime API method was added",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::shape::{Projection, ProjectionField, ShapeHash};
    use blake2::Digest;
    use rtao_core::codec::Value;

    const METADATA: &[u8] = include_bytes!("../../../vendor/metadata/finney/479.v15.scale");

    fn parse_core() -> Runtime {
        Runtime::parse(METADATA, 479, 1, 42).unwrap()
    }

    fn runtime() -> RuntimeHandle {
        RuntimeHandle::new(parse_core()).unwrap()
    }

    fn pinned_api_versions() -> std::collections::BTreeMap<String, u32> {
        crate::generated::runtime_apis::ALL
            .iter()
            .map(|api| (api.api.into(), api.version))
            .collect()
    }

    #[test]
    fn all_generated_descriptors_match_the_pinned_live_metadata() {
        let runtime =
            RuntimeHandle::new_with_api_versions(parse_core(), pinned_api_versions()).unwrap();
        let report = verify_surface(&runtime);
        assert_eq!(
            report.checked,
            crate::generated::shapes::USED.len() + crate::generated::shapes::models().len()
        );
        assert!(
            report.checked > 1000,
            "generated used surface is unexpectedly empty or incomplete"
        );
        assert!(report.changes.is_empty(), "{:#?}", report.changes);
        assert!(report.unverified_api_versions.is_empty());
        assert!(std::ptr::eq(
            runtime.surface_report(),
            runtime.clone().surface_report()
        ));
    }

    #[test]
    fn unrelated_added_call_is_additive_and_existing_wrappers_still_compose() {
        let mut core = parse_core();
        let call_type = core.pallet("SubtensorModule").unwrap().calls_type.unwrap();
        let ty = core
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == call_type)
            .unwrap();
        let TypeDef::Variant(calls) = &mut ty.ty.type_def else {
            unreachable!()
        };
        let free_index = (0..=255)
            .find(|index| calls.variants.iter().all(|call| call.index != *index))
            .unwrap();
        let mut additional = calls.variants.first().unwrap().clone();
        additional.index = free_index;
        additional.name = "rtao_test_added_call".into();
        additional.fields.clear();
        calls.variants.push(additional);
        let runtime = RuntimeHandle::new_with_api_versions(core, pinned_api_versions()).unwrap();
        let report = verify_surface(&runtime);
        assert_eq!(report.additive_items(), 1, "{:#?}", report.changes);
        assert_eq!(report.breaking_items(), 0, "{:#?}", report.changes);
        assert!(report.shapes_match());
        let inner = crate::generated::calls::system::remark(vec![1, 2, 3])
            .compose(&runtime)
            .unwrap();
        let wrapped = crate::generated::calls::sudo::sudo(inner)
            .compose(&runtime)
            .unwrap();
        assert_eq!(wrapped.stamp(), runtime.stamp());
    }

    fn descriptor(
        runtime: &RuntimeHandle,
        pallet: &'static str,
        name: &'static str,
    ) -> &'static CallDescriptor {
        let calls_type = runtime.core().pallet(pallet).unwrap().calls_type.unwrap();
        let call = variants(runtime.core(), Some(calls_type), "call")
            .unwrap()
            .iter()
            .find(|call| call.name == name)
            .unwrap();
        let fields = call
            .fields
            .iter()
            .map(|field| FieldDescriptor {
                name: Box::leak(field.name.clone().unwrap_or_default().into_boxed_str()),
                type_ident: Box::leak(runtime.core().type_ident(field.ty.id).into_boxed_str()),
            })
            .collect::<Vec<_>>();
        Box::leak(Box::new(CallDescriptor {
            pallet,
            name,
            fields: Box::leak(fields.into_boxed_slice()),
            shape: runtime.fields_shape(&call.fields).unwrap(),
            pallet_index: runtime.pallet_index(pallet).unwrap(),
            call_index: call.index,
        }))
    }

    fn remark(expected: &'static CallDescriptor) -> Call {
        Call::new(
            expected,
            Value::record(vec![("remark".into(), Value::Bytes(vec![1, 2, 3]))]),
            Vec::new(),
        )
    }

    #[test]
    fn call_uses_core_encoder_and_records_both_versions() {
        let runtime = runtime();
        let descriptor = descriptor(&runtime, "System", "remark");
        let call = remark(descriptor);
        let expected = runtime
            .core()
            .compose_call("System", "remark", &call.args)
            .unwrap();
        let composed = call.compose(&runtime).unwrap();
        assert_eq!(composed.as_bytes(), expected);
        assert_eq!(composed.stamp(), runtime.stamp());
        let expected_hash: [u8; 32] =
            blake2::Blake2b::<blake2::digest::consts::U32>::digest(&expected).into();
        assert_eq!(composed.hash(), expected_hash);
    }

    #[test]
    fn mismatch_refuses_only_the_changed_call() {
        let runtime = runtime();
        let good = descriptor(&runtime, "System", "remark");
        let mut bad = *good;
        bad.shape = ShapeHash([255; 32]);
        let bad = Box::leak(Box::new(bad));
        let error = remark(bad).compose(&runtime).unwrap_err();
        assert!(matches!(error, ComposeError::SurfaceChanged { .. }));
        assert!(remark(good).compose(&runtime).is_ok());
        let report = verify_surface_with(&runtime, &[SurfaceDescriptor::Call(bad)]);
        assert_eq!(report.breaking_items(), 1);
        assert!(!report.shape_matches("call:System.remark"));
    }

    #[test]
    fn descriptors_outside_the_verified_set_fail_closed() {
        let runtime = runtime();
        let expected = descriptor(&runtime, "System", "remark");
        assert!(!SurfaceReport::default().shape_matches("call:System.remark"));
        assert!(!verify_surface_with(&runtime, &[]).shape_matches("call:System.remark"));
        let report = verify_surface_with(&runtime, &[SurfaceDescriptor::Call(expected)]);
        assert!(report.shape_matches("call:System.remark"));
        assert!(!report.shape_matches("call:System.set_code"));
        assert!(!report.shape_matches("model:Unknown"));
    }

    #[test]
    fn moved_index_requires_regeneration_and_encoder_then_uses_the_live_index() {
        let baseline = runtime();
        let expected = descriptor(&baseline, "System", "remark");
        let mut core = parse_core();
        let free_index = (0..=255)
            .find(|index| core.pallets.iter().all(|pallet| pallet.index != *index))
            .unwrap();
        core.pallets
            .iter_mut()
            .find(|pallet| pallet.name == "System")
            .unwrap()
            .index = free_index;
        let changed = RuntimeHandle::new(core).unwrap();
        let report = verify_surface_with(&changed, &[SurfaceDescriptor::Call(expected)]);
        assert_eq!(report.breaking_items(), 1);
        assert!(report.shapes_match());
        assert!(matches!(
            remark(expected).compose(&changed),
            Err(ComposeError::SurfaceChanged { .. })
        ));
        let mut regenerated = *expected;
        regenerated.pallet_index = free_index;
        let regenerated = Box::leak(Box::new(regenerated));
        let encoded = remark(regenerated).compose(&changed).unwrap();
        assert_eq!(encoded.as_bytes().first(), Some(&free_index));
    }

    #[test]
    fn stale_nested_call_stamp_refuses_composition() {
        let runtime = runtime();
        let expected = descriptor(&runtime, "System", "remark");
        let mut call = remark(expected);
        let mut stale = runtime.stamp();
        stale.transaction_version += 1;
        call.nested_stamps.push(stale);
        assert!(matches!(
            call.compose(&runtime),
            Err(ComposeError::VersionMismatch { .. })
        ));
        stale = runtime.stamp();
        stale.spec_version += 1;
        call.nested_stamps.clear();
        call.nested_stamps.push(stale);
        assert!(matches!(
            call.compose(&runtime),
            Err(ComposeError::VersionMismatch { .. })
        ));
    }

    #[test]
    fn api_version_absence_and_changes_are_reported_explicitly() {
        let runtime = runtime();
        let method = runtime
            .runtime_api("AccountNonceApi", "account_nonce")
            .unwrap();
        let inputs = method
            .inputs
            .iter()
            .map(|input| FieldDescriptor {
                name: Box::leak(input.name.clone().into_boxed_str()),
                type_ident: Box::leak(runtime.core().type_ident(input.ty).into_boxed_str()),
            })
            .collect::<Vec<_>>();
        let mut parts = method
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), runtime.exact_shape(input.ty).unwrap()))
            .collect::<Vec<_>>();
        parts.push(("$output", runtime.exact_shape(method.output).unwrap()));
        let expected = ApiSurface {
            api: "AccountNonceApi",
            name: "account_nonce",
            inputs: Box::leak(inputs.into_boxed_slice()),
            output_type_ident: Box::leak(runtime.core().type_ident(method.output).into_boxed_str()),
            version: 1,
            shape: shape_parts(&parts),
            output_projection: None,
        };
        let descriptors = [SurfaceDescriptor::RuntimeApi(expected)];
        let report = verify_surface_with(&runtime, &descriptors);
        assert_eq!(report.unverified_api_versions.len(), 1);
        assert!(report.shapes_match());
        let versions = [("AccountNonceApi".into(), 1)].into_iter().collect();
        let versioned = RuntimeHandle::new_with_api_versions(parse_core(), versions).unwrap();
        let report = verify_surface_with(&versioned, &descriptors);
        assert!(report.unverified_api_versions.is_empty());
        assert!(report.changes.is_empty());
        let versions = [("AccountNonceApi".into(), 2)].into_iter().collect();
        let changed = RuntimeHandle::new_with_api_versions(parse_core(), versions).unwrap();
        let report = verify_surface_with(&changed, &descriptors);
        assert_eq!(report.behavioural_items(), 1);
        assert!(report.unverified_api_versions.is_empty());
    }

    #[test]
    fn storage_behaviour_changes_do_not_change_the_typed_shape() {
        let baseline = runtime();
        let storage = baseline.storage_entry("System", "Account").unwrap();
        let mut types = storage.key_types.clone();
        types.push(storage.value_type);
        let expected = StorageSurface {
            pallet: "System",
            name: "Account",
            prefix: Box::leak(storage.prefix.clone().into_boxed_str()),
            key_type_idents: Box::leak(
                storage
                    .key_types
                    .iter()
                    .map(|ty| &*Box::leak(baseline.core().type_ident(*ty).into_boxed_str()))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            value_type_ident: Box::leak(
                baseline
                    .core()
                    .type_ident(storage.value_type)
                    .into_boxed_str(),
            ),
            hashers: Box::leak(
                storage
                    .hashers
                    .iter()
                    .map(|hasher| &*Box::leak(hasher.clone().into_boxed_str()))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            modifier: Box::leak(storage.modifier.clone().into_boxed_str()),
            default_bytes: Box::leak(storage.default_bytes.clone().into_boxed_slice()),
            shape: baseline.tuple_shape(&types).unwrap(),
        };
        let mut core = parse_core();
        let storage = core
            .pallets
            .iter_mut()
            .find(|pallet| pallet.name == "System")
            .unwrap()
            .storage
            .iter_mut()
            .find(|storage| storage.name == "Account")
            .unwrap();
        storage.default_bytes.push(0);
        storage.prefix = "DifferentPrefix".into();
        storage.hashers.fill("Identity".into());
        let changed = RuntimeHandle::new(core).unwrap();
        let report = verify_surface_with(&changed, &[SurfaceDescriptor::Storage(expected)]);
        assert_eq!(report.behavioural_items(), 1);
        assert_eq!(report.breaking_items(), 0);
        assert!(report.shapes_match());
        assert_eq!(report.changes.len(), 3);
    }

    #[test]
    fn appended_model_fields_preserve_projection_but_changed_declared_fields_refuse() {
        let baseline = runtime();
        let storage = baseline.storage_entry("System", "Account").unwrap();
        let type_id = storage.value_type;
        let projection = Projection::Composite(vec![ProjectionField::exact("nonce")]);
        let path = baseline
            .core()
            .resolve(type_id)
            .unwrap()
            .path
            .segments
            .join("::");
        let expected = ModelSurface {
            type_path: Box::leak(path.into_boxed_str()),
            shape: baseline.projection_shape(type_id, &projection).unwrap(),
            projection,
        };
        let mut core = parse_core();
        let ty = core
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == type_id)
            .unwrap();
        let TypeDef::Composite(composite) = &mut ty.ty.type_def else {
            unreachable!()
        };
        let mut appended = composite.fields.first().unwrap().clone();
        appended.name = Some("new_unrelated_field".into());
        composite.fields.push(appended);
        let appended = RuntimeHandle::new(core).unwrap();
        assert!(
            verify_surface_with(&appended, &[SurfaceDescriptor::Model(expected.clone())])
                .shapes_match()
        );
        let mut core = parse_core();
        let ty = core
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == type_id)
            .unwrap();
        let TypeDef::Composite(composite) = &mut ty.ty.type_def else {
            unreachable!()
        };
        composite
            .fields
            .iter_mut()
            .find(|field| field.name.as_deref() == Some("nonce"))
            .unwrap()
            .name = Some("renamed_nonce".into());
        let renamed = RuntimeHandle::new(core).unwrap();
        let report = verify_surface_with(&renamed, &[SurfaceDescriptor::Model(expected)]);
        assert_eq!(report.behavioural_items(), 1);
        assert!(!report.shapes_match());
    }
}
