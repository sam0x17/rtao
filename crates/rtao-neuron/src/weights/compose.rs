//! Weight call composition when a compiled descriptor no longer matches (D-088).
//!
//! Each attempt composes the one call its mechanism uses. The compiled
//! descriptor is used whenever its exact shape still matches. Otherwise an
//! installed [`WeightCallEncoder`] composes the call, or, without one, the
//! call is resolved by name against live metadata and validated by
//! structural class ([`rtao_runtime::compose_resolved`]). Only a call that
//! none of these can produce pauses the mechanism. Every path ends in the
//! same executor: preflight, policy, the semantic guard, journal binding and
//! reconciliation, shielding and signing are unchanged.

use rtao_client::journal::Incompatibility;
use rtao_primitives::{MechId, NetUid};
use rtao_runtime::{
    ArgClass, ArgValue, CallBytes, CallDescriptor, ComposeError, ResolvedArg, RuntimeHandle,
    calls::subtensor_module as calls,
};
use std::sync::Arc;

/// Pallet of every weight call.
pub const WEIGHT_PALLET: &str = "SubtensorModule";

/// Direct-row weight calls an application encoder may return.
pub const SET_WEIGHT_CALLS: [&str; 2] = ["set_mechanism_weights", "set_weights"];

/// Timelocked-commit weight calls an application encoder may return.
pub const COMMIT_WEIGHT_CALLS: [&str; 2] = [
    "commit_timelocked_mechanism_weights",
    "commit_timelocked_weights",
];

/// One weight call the scheduler is about to sign, as an application encoder
/// receives it. Values are final: the row is conformed and the commitment is
/// encrypted and tagged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightCall<'a> {
    /// Set a mechanism's row directly.
    Set {
        /// Subnet.
        netuid: NetUid,
        /// Mechanism.
        mechanism: MechId,
        /// Destination UIDs in submission order.
        dests: &'a [u16],
        /// Chain-native weights parallel to `dests`.
        weights: &'a [u16],
        /// Weights version key.
        version_key: u64,
    },
    /// Commit a timelocked row.
    Commit {
        /// Subnet.
        netuid: NetUid,
        /// Mechanism.
        mechanism: MechId,
        /// Encrypted payload.
        commit: &'a [u8],
        /// Drand round that reveals the payload.
        reveal_round: u64,
        /// Tag the runtime requires on the commit.
        commit_reveal_version: u16,
    },
}

impl WeightCall<'_> {
    /// Subnet of this call.
    pub fn netuid(&self) -> NetUid {
        match self {
            Self::Set { netuid, .. } | Self::Commit { netuid, .. } => *netuid,
        }
    }
    /// Mechanism of this call.
    pub fn mechanism(&self) -> MechId {
        match self {
            Self::Set { mechanism, .. } | Self::Commit { mechanism, .. } => *mechanism,
        }
    }
    fn is_commit(&self) -> bool {
        matches!(self, Self::Commit { .. })
    }
    fn arguments(&self) -> Vec<ResolvedArg<'static>> {
        let arg = |name, class, value| ResolvedArg { name, class, value };
        match *self {
            Self::Set {
                netuid,
                mechanism,
                dests,
                weights,
                version_key,
            } => vec![
                arg(
                    "netuid",
                    ArgClass::Uint(16),
                    ArgValue::Uint(netuid.0.into()),
                ),
                arg(
                    "mecid",
                    ArgClass::Uint(8),
                    ArgValue::Uint(mechanism.0.into()),
                ),
                arg("dests", u16_seq(), ArgValue::uints(dests)),
                arg("weights", u16_seq(), ArgValue::uints(weights)),
                arg(
                    "version_key",
                    ArgClass::Uint(64),
                    ArgValue::Uint(version_key.into()),
                ),
            ],
            Self::Commit {
                netuid,
                mechanism,
                commit,
                reveal_round,
                commit_reveal_version,
            } => vec![
                arg(
                    "netuid",
                    ArgClass::Uint(16),
                    ArgValue::Uint(netuid.0.into()),
                ),
                arg(
                    "mecid",
                    ArgClass::Uint(8),
                    ArgValue::Uint(mechanism.0.into()),
                ),
                arg("commit", ArgClass::Bytes, ArgValue::Bytes(commit.to_vec())),
                arg(
                    "reveal_round",
                    ArgClass::Uint(64),
                    ArgValue::Uint(reveal_round.into()),
                ),
                arg(
                    "commit_reveal_version",
                    ArgClass::Uint(16),
                    ArgValue::Uint(commit_reveal_version.into()),
                ),
            ],
        }
    }
}

fn u16_seq() -> ArgClass {
    ArgClass::Seq(Box::new(ArgClass::Uint(16)))
}

/// Application-supplied weight call encoder (D-088).
///
/// The scheduler uses it only for a call whose compiled descriptor no longer
/// matches the live runtime, before name-resolved composition. The returned
/// bytes are never trusted: they are imported strictly under the signing
/// runtime (complete, canonical, re-encoded through the one core encoder),
/// then must be a `SubtensorModule` weight call ([`SET_WEIGHT_CALLS`] or
/// [`COMMIT_WEIGHT_CALLS`] for the requested kind, or one of
/// [`Self::additional_calls`] when a semantic guard is installed), must not
/// carry a nested runtime call, must name the scheduler's subnet and must
/// carry the prepared mechanism, row or commitment, reveal round, version
/// key and commit-reveal tag wherever the live call has those fields. A
/// refusal pauses that mechanism's attempt and nothing is signed.
pub trait WeightCallEncoder: Send + Sync {
    /// Stable contract token, recorded with durable work. Work recorded under
    /// a token is only rebuilt while an encoder with that token is installed.
    fn contract(&self) -> &str;
    /// Encode `call` as complete SCALE call bytes under `runtime`.
    fn encode(
        &self,
        runtime: &RuntimeHandle,
        call: &WeightCall<'_>,
    ) -> Result<Vec<u8>, Incompatibility>;
    /// Further `SubtensorModule` call names this encoder may return. They are
    /// accepted only while a semantic guard is installed.
    fn additional_calls(&self) -> &[&str] {
        &[]
    }
}

/// How one attempt composes its call.
#[derive(Clone)]
pub(super) enum Composer {
    /// The compiled descriptor matches the live call.
    Compiled,
    /// Name-resolved composition validated by structural class.
    Resolved,
    /// The installed application encoder, whose bytes are validated.
    Application(Arc<dyn WeightCallEncoder>),
}

impl Composer {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Compiled => "compiled",
            Self::Resolved => "name-resolved",
            Self::Application(_) => "application encoder",
        }
    }
}

/// Compiled descriptor of the call a mechanism uses.
pub(super) fn compiled(commit: bool) -> &'static CallDescriptor {
    if commit {
        &calls::COMMIT_TIMELOCKED_MECHANISM_WEIGHTS
    } else {
        &calls::SET_MECHANISM_WEIGHTS
    }
}

/// Argument classes of the call a mechanism uses.
pub(super) fn classes(commit: bool) -> Vec<(&'static str, ArgClass)> {
    let sample = if commit {
        WeightCall::Commit {
            netuid: NetUid(0),
            mechanism: MechId(0),
            commit: &[],
            reveal_round: 0,
            commit_reveal_version: 0,
        }
    } else {
        WeightCall::Set {
            netuid: NetUid(0),
            mechanism: MechId(0),
            dests: &[],
            weights: &[],
            version_key: 0,
        }
    };
    sample
        .arguments()
        .into_iter()
        .map(|arg| (arg.name, arg.class))
        .collect()
}

/// Whether the compiled descriptor of this call still matches `runtime`.
pub(super) fn compiled_matches(runtime: &RuntimeHandle, commit: bool) -> bool {
    let call = compiled(commit);
    runtime
        .surface_report()
        .shape_matches(&format!("call:{}.{}", call.pallet, call.name))
}

/// Whether name-resolved composition accepts this call under `runtime`.
pub(super) fn check_resolved(runtime: &RuntimeHandle, commit: bool) -> Result<(), ComposeError> {
    let call = compiled(commit);
    rtao_runtime::check_resolved(runtime, call.pallet, call.name, &classes(commit))
}

/// Select how to compose this call, or explain why it cannot be composed.
pub(super) fn select(
    runtime: &RuntimeHandle,
    commit: bool,
    encoder: Option<&Arc<dyn WeightCallEncoder>>,
) -> Result<Composer, String> {
    if compiled_matches(runtime, commit) {
        return Ok(Composer::Compiled);
    }
    if let Some(encoder) = encoder {
        return Ok(Composer::Application(encoder.clone()));
    }
    check_resolved(runtime, commit)
        .map(|()| Composer::Resolved)
        .map_err(|error| format!("name-resolved composition refused: {error}"))
}

/// Compose by name under `runtime`.
pub(super) fn compose_resolved(
    runtime: &RuntimeHandle,
    call: &WeightCall<'_>,
) -> Result<CallBytes, ComposeError> {
    let descriptor = compiled(call.is_commit());
    rtao_runtime::compose_resolved(
        runtime,
        descriptor.pallet,
        descriptor.name,
        &call.arguments(),
    )
}

fn refusal(code: &str, descriptor: Option<String>, detail: impl Into<String>) -> Incompatibility {
    Incompatibility {
        code: code.into(),
        descriptor,
        detail: detail.into(),
        extensions: Default::default(),
    }
}

/// Classes of every weight field the scheduler prepares.
fn known_fields() -> Vec<(&'static str, ArgClass)> {
    let mut fields = classes(false);
    fields.extend(classes(true).into_iter().skip(2));
    fields
}

/// Encode through the application encoder and validate the returned bytes.
///
/// `guarded` reports whether a semantic guard is installed, which
/// [`WeightCallEncoder::additional_calls`] requires.
pub(super) fn encode_application(
    encoder: &dyn WeightCallEncoder,
    runtime: &RuntimeHandle,
    call: &WeightCall<'_>,
    guarded: bool,
) -> Result<CallBytes, Incompatibility> {
    let bytes = encoder.encode(runtime, call)?;
    let imported = CallBytes::decode(&bytes, runtime).map_err(|error| {
        refusal(
            "weight-encoder-bytes",
            None,
            format!("encoded call does not import under the signing runtime: {error}"),
        )
    })?;
    let inspected = rtao_runtime::inspect_call(runtime, &imported, &known_fields())
        .map_err(|error| refusal("weight-encoder-shape", None, error.to_string()))?;
    let descriptor = Some(format!("call:{}.{}", inspected.pallet, inspected.call));
    let refuse = |code: &str, detail: String| refusal(code, descriptor.clone(), detail);
    let known = if call.is_commit() {
        COMMIT_WEIGHT_CALLS.as_slice()
    } else {
        SET_WEIGHT_CALLS.as_slice()
    };
    let is_known = known.contains(&inspected.call.as_str());
    let declared = encoder
        .additional_calls()
        .contains(&inspected.call.as_str());
    if inspected.pallet != WEIGHT_PALLET || !(is_known || declared) {
        return Err(refuse(
            "weight-encoder-call",
            format!(
                "{}.{} is not a {} weight call",
                inspected.pallet,
                inspected.call,
                if call.is_commit() {
                    "commit"
                } else {
                    "direct-row"
                }
            ),
        ));
    }
    if !is_known && !guarded {
        return Err(refuse(
            "weight-encoder-call",
            format!(
                "{} is declared by the encoder but requires an installed semantic guard",
                inspected.call
            ),
        ));
    }
    if let Some(field) = inspected.fields.iter().find(|field| field.nests_call) {
        return Err(refuse(
            "weight-encoder-nested",
            format!("field {} can carry a nested runtime call", field.name),
        ));
    }
    let expected = call.arguments();
    for arg in &expected {
        let present = inspected
            .field(arg.name)
            .and_then(|field| field.value.as_ref());
        let required = match arg.name {
            "netuid" => true,
            // A call without a mechanism field targets mechanism zero.
            "mecid" => call.mechanism().0 != 0,
            "dests" | "weights" | "commit" | "reveal_round" => is_known,
            _ => false,
        };
        match present {
            Some(value) if *value != arg.value => {
                return Err(refuse(
                    "weight-encoder-value",
                    format!("field {} differs from the prepared value", arg.name),
                ));
            }
            None if required => {
                return Err(refuse(
                    "weight-encoder-value",
                    format!("field {} is missing", arg.name),
                ));
            }
            _ => {}
        }
    }
    // A field the prepared call does not carry (a row on a commit, or the
    // reverse) must not appear at all.
    for (name, _) in known_fields() {
        if expected.iter().all(|arg| arg.name != name) && inspected.field(name).is_some() {
            return Err(refuse(
                "weight-encoder-value",
                format!("field {name} does not belong to this weight call"),
            ));
        }
    }
    Ok(imported)
}
