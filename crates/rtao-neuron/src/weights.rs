// Derived from RaoFoundation/subtensor pallets/subtensor/src/utils/misc.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Coherent best-head weight preparation and actual-epoch reveal tracking.

mod compose;
mod durable;
#[cfg(test)]
mod read_tests;
mod reveal;
mod slot;
mod submit;
#[cfg(test)]
mod tests;

use crate::{Clock, ClockGuard, NeuronCx, StateCell, Validator, reads};
use blake2::{Blake2b, Digest, digest::consts::U32};
pub use compose::{
    COMMIT_WEIGHT_CALLS, SET_WEIGHT_CALLS, WEIGHT_PALLET, WeightCall, WeightCallEncoder,
};
use futures::StreamExt;
pub use reveal::{EpochNotice, PendingReveal, RevealMissReason, RevealOutcome, RevealWatcher};
use rtao_client::{BlockHeader, BlockRef, Client, Finality, Snapshot};
use rtao_core::{DynValue, codec::Value, timelock::epoch_schedule::EpochScheduleState};
use rtao_intents::weights::{
    CommitSizeLimit, ConformState, TimelockCommitRequest, TimelockCommitter, conform,
};
use rtao_primitives::{AccountId, IntoCoreValue, MechId, NetUid, NetUidStorageIndex, Uid};
use rtao_runtime::storage::subtensor_module as st;
use rtao_signer::KeyView;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Timelocked payload format this build encodes (D-086).
///
/// Version 4 is the `WeightsTlockPayload` SCALE struct (hotkey, UIDs, values,
/// version key) encrypted to a drand quicknet round. Runtime code decides which
/// payload format a reveal decodes; the `CommitRevealWeightsVersion` storage
/// value is only a tag that each commit must repeat. See
/// [`REVIEWED_CR_RUNTIME_CODE`] and [`RevealWatcher::reveals_native_payload`]
/// for when the scheduler follows a different stored tag.
pub const NATIVE_CR_PAYLOAD_VERSION: u16 = 4;

/// Former name of [`NATIVE_CR_PAYLOAD_VERSION`].
#[deprecated(note = "use NATIVE_CR_PAYLOAD_VERSION; the commit tag follows the chain under D-086")]
pub const SUPPORTED_CR_VERSION: u16 = NATIVE_CR_PAYLOAD_VERSION;

/// Runtime code reviewed to reveal the [`NATIVE_CR_PAYLOAD_VERSION`] payload,
/// as `(spec_version, blake2_256 of :code)` from `docs/compat/vNNN-provenance.json`.
///
/// Each entry's pinned `coinbase/reveal_commits.rs` decodes
/// `WeightsTlockPayload` (freeze_struct `b6833b5029be4127`) and never reads
/// `CommitRevealWeightsVersion`. Under exactly this code the scheduler can tag
/// commits with whatever version root stores. Unlisted code is followed only
/// after one of this validator's own commitments revealed under it.
pub const REVIEWED_CR_RUNTIME_CODE: [(u32, &str); 2] = [
    (
        484,
        "0x23041f50d3529b359e9be3b90f3ca40474678e834e97f49fb401d0c691097760",
    ),
    (
        486,
        "0x49781ac4cfda0ac6cf6e53cfceea7c728394d9c28da7881f47e3fbfbc239800f",
    ),
];

/// Re-selections of the commit tag after fee-free `IncorrectCommitRevealVersion`
/// dispatch rejections within one attempt. Each waits one block, doubling to
/// eight, then re-reads the preflight (D-086).
pub const CR_VERSION_RETRIES: u32 = 10;

/// Wait before retry `attempt` (from zero): one block, doubling to eight.
fn block_backoff(block_time: f64, attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs_f64(block_time).saturating_mul(1 << attempt.min(3))
}

/// Spec version of a reviewed runtime whose `:code` hash is `code`.
pub fn reviewed_cr_runtime(code: rtao_primitives::H256) -> Option<u32> {
    let code = format!("0x{}", hex::encode(code.0));
    REVIEWED_CR_RUNTIME_CODE
        .iter()
        .find(|(_, hash)| *hash == code)
        .map(|(spec, _)| *spec)
}

/// Storage hash of `:code` at `block`, which identifies the runtime code that
/// executes the block's children.
pub(crate) async fn runtime_code_hash(
    client: &Client,
    block: rtao_primitives::H256,
) -> anyhow::Result<rtao_primitives::H256> {
    let reply: Option<String> = client
        .request(
            "state_getStorageHash",
            serde_json::json!(["0x3a636f6465", format!("0x{}", hex::encode(block.0))]),
        )
        .await?;
    let bytes = reply
        .as_deref()
        .and_then(|text| text.strip_prefix("0x"))
        .and_then(|text| hex::decode(text).ok())
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| anyhow::anyhow!("runtime code identity is unavailable at this block"))?;
    Ok(rtao_primitives::H256(bytes))
}

/// Pure scoring result, evaluated only after one coherent preflight.
#[derive(Clone, Debug, PartialEq)]
pub enum WeightsProposal {
    /// Prepare and submit a new row.
    Set(WeightsInput),
    /// Stable application work. Repeated callback delivery uses the same ID.
    Operation {
        /// Application operation identity; never reuse it for changed input.
        id: String,
        /// Opaque application correlation identity.
        correlation_id: Option<String>,
        /// Intended row, retained across missed epochs and restart.
        input: WeightsInput,
    },
    /// Retain the previous chain row.
    HoldPrevious,
    /// Skip this opportunity with an operator-visible reason.
    Skip(&'static str),
}

/// Destinations and scores accepted by a validator's pure weights callback.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WeightsInput {
    /// Deterministically ordered UID-to-score mapping.
    ByUid(BTreeMap<Uid, f64>),
    /// Parallel destination and relative-score vectors.
    Parallel {
        /// Destinations in submission order.
        uids: Vec<Uid>,
        /// Finite, nonnegative relative scores.
        weights: Vec<f64>,
    },
    /// Resolve account identities using the current last-good graph.
    ByHotkey(Vec<(AccountId, f64)>),
    /// Preserve every integer UID/value pair under a sole-permit Null consensus proof.
    /// Chain count and configured share limits apply without scaling or dropping zeros.
    RawU16 {
        /// Destinations in submission order.
        uids: Vec<Uid>,
        /// Chain-native integer ratios.
        values: Vec<u16>,
    },
}

/// What the scheduler does with a prepared row whose positive weight goes only
/// to this validator's UID or to the subnet owner hotkey's UID (D-093).
///
/// It is chosen per proposal ([`WeightScheduler::on_finalized_with`],
/// [`crate::Validator::owner_row_policy`]) or per queued operation
/// ([`WeightScheduler::enqueue_with_owner_rows`]). A durable scheduler
/// journals it with the operation, so restart, recovery and re-preparation
/// decide the same way.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OwnerRowPolicy {
    /// Keep the previous chain row and report
    /// [`WeightAttemptOutcome::HoldPrevious`]; nothing is signed. Earlier
    /// releases always applied this guard.
    #[default]
    Hold,
    /// Submit a row whose every positive entry goes to the owner hotkey's UID
    /// (an owner burn row) through the normal path: a direct row, or a
    /// timelocked commit and its automatic reveal.
    ///
    /// This validator's own UID counts as the owner's only when its hotkey is
    /// the owner hotkey. These rows still hold: a row with positive weight on
    /// a non-owner own UID (self-only, or self plus owner), and a row with no
    /// positive entry.
    Submit,
}
impl OwnerRowPolicy {
    /// Whether the scheduler keeps the previous chain row instead of
    /// submitting `row`, given as prepared `(UID, value)` pairs; zero values
    /// are ignored.
    ///
    /// `own` is this validator's UID, and `is_owner(uid)` reports whether
    /// `uid` belongs to the subnet owner hotkey. For `own` it should answer
    /// from this validator's own hotkey (whether it is the owner hotkey)
    /// rather than from a possibly stale UID map, as the scheduler does. A
    /// row with a positive entry on any other UID is never held.
    ///
    /// ```
    /// use rtao_neuron::weights::OwnerRowPolicy;
    /// use rtao_primitives::Uid;
    /// let (own, owner) = (Uid(0), Uid(2));
    /// let burn = [(Uid(1), 0), (owner, u16::MAX)];
    /// assert!(OwnerRowPolicy::Hold.holds(burn, own, |uid| uid == owner));
    /// assert!(!OwnerRowPolicy::Submit.holds(burn, own, |uid| uid == owner));
    /// // A self-only row keeps holding under either policy.
    /// assert!(OwnerRowPolicy::Submit.holds([(own, 1)], own, |uid| uid == owner));
    /// ```
    pub fn holds(
        self,
        row: impl IntoIterator<Item = (Uid, u16)>,
        own: Uid,
        mut is_owner: impl FnMut(Uid) -> bool,
    ) -> bool {
        let (mut positive, mut own_entry) = (false, false);
        for (dest, value) in row {
            if value == 0 {
                continue;
            }
            positive = true;
            // The owner check comes first: an owner validator's own UID is
            // the owner's UID.
            if is_owner(dest) {
                continue;
            }
            if dest == own {
                own_entry = true;
                continue;
            }
            // Another destination: the guard never applies.
            return false;
        }
        match self {
            Self::Hold => true,
            Self::Submit => !positive || own_entry,
        }
    }

    /// Serde skip predicate: the default is omitted from journaled effects.
    const fn is_hold(&self) -> bool {
        matches!(self, Self::Hold)
    }
}

/// Explicit weight version; never follows storage and defeats upgrade policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VersionKey {
    /// Python v11's default.
    #[default]
    Zero,
    /// Operator-selected version supported by the scoring implementation.
    Fixed(u64),
}
impl VersionKey {
    /// Value carried by both direct rows and encrypted payloads.
    pub const fn value(self) -> u64 {
        match self {
            Self::Zero => 0,
            Self::Fixed(value) => value,
        }
    }
}

/// Weight opportunity cadence, independent of the scoring callback cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightCadence {
    /// One attempt in each actual subnet epoch after this offset.
    OncePerEpoch {
        /// Minimum blocks after the observed epoch boundary.
        offset_blocks: u64,
    },
    /// Wait for the chain rate limit plus an operator margin.
    RateLimited {
        /// Extra blocks beyond the chain rate limit.
        margin: u64,
    },
}

/// Validator-specific submission policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeightPolicy {
    /// Explicit supported weight version.
    pub version_key: VersionKey,
    /// Override the default CR epoch jitter or direct-row rate limit.
    pub cadence: Option<WeightCadence>,
}

/// Certain preflight rejection with a shared semantic code and remediation.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{name}: {remediation}")]
pub struct WeightRefusal {
    /// Semantic chain-error category.
    pub code: rtao_catalog::ErrorCode,
    /// Specific failed check.
    pub name: &'static str,
    /// Action needed before another attempt can succeed.
    pub remediation: String,
}

/// Observable result of the latest mechanism scheduling opportunity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeightAttemptOutcome {
    /// Cadence, rate limit or unresolved submission prevented work.
    Scheduled,
    /// The chain rate limit schedules this row without marking readiness failed.
    RateLimited {
        /// Earliest best height allowed by the current chain limit.
        retry_after_block: u64,
        /// Explanation for the next scheduling opportunity.
        remediation: String,
    },
    /// The application, or the self/owner guard under the proposal's
    /// [`OwnerRowPolicy`], retained the existing row.
    HoldPrevious,
    /// The application supplied a reason to skip this opportunity.
    Skipped(String),
    /// A certain chain rejection was detected before signing.
    Refused(WeightRefusal),
    /// Runtime call shape changed; this mechanism resumes after verification.
    PausedSurface {
        /// Full name of the incompatible descriptor.
        descriptor: String,
        /// Operator remediation and structural difference.
        detail: String,
    },
    /// The row reached finalized inclusion.
    Submitted {
        /// Immutable finalized inclusion identity.
        block_hash: rtao_primitives::H256,
        /// Inclusion height.
        block_number: u64,
    },
    /// Fate remains unknown and requires reconciliation before another row.
    Unresolved(String),
    /// RPC, preparation or a definite submission failed.
    Failed(String),
}
impl WeightAttemptOutcome {
    /// Shared semantic category for an actionable refusal or scheduled rate limit.
    pub fn code(&self) -> Option<rtao_catalog::ErrorCode> {
        match self {
            Self::Refused(refusal) => Some(refusal.code),
            Self::RateLimited { .. } => Some(rtao_catalog::ErrorCode::RateLimited),
            Self::PausedSurface { .. } => Some(rtao_catalog::ErrorCode::PolicyViolation),
            Self::Unresolved(_) | Self::Failed(_) => Some(rtao_catalog::ErrorCode::Internal),
            Self::Scheduled | Self::HoldPrevious | Self::Skipped(_) | Self::Submitted { .. } => {
                None
            }
        }
    }
}

/// Retained scheduling result available without inspecting tracing logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightAttempt {
    /// Mechanism evaluated by the scheduler.
    pub mechanism: MechId,
    /// Best height used for the coherent preflight.
    pub best_block: u64,
    /// Latest scheduling or finalized-submission outcome.
    pub outcome: WeightAttemptOutcome,
}

/// Next scheduling action under one observed chain view, not signing authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeightOpportunity {
    /// Cadence permits preparation; runtime, policy and semantic checks still apply.
    Ready,
    /// New submissions are paused; continue finalized observation.
    Paused,
    /// Reconcile a retained attempt or continue bounded inclusion/reveal observation.
    Reconcile,
    /// Recheck chain limits at or after this best height.
    RateLimited {
        /// Earliest height under the currently observed chain limit.
        retry_after_block: u64,
    },
    /// Wait for an observed epoch newer than this one, then apply its offset.
    /// No wall-clock guarantee is implied by a predicted epoch boundary.
    AwaitEpoch {
        /// Last epoch already used by this scheduler.
        after_epoch: u64,
        /// Required elapsed blocks inside a newly observed epoch.
        offset_blocks: u64,
    },
    /// Recheck the offset at or after this best height.
    AwaitBlock {
        /// Earliest height under the currently observed epoch state.
        not_before_block: u64,
    },
}
fn refuse(
    code: rtao_catalog::ErrorCode,
    name: &'static str,
    remediation: impl Into<String>,
) -> anyhow::Error {
    WeightRefusal {
        code,
        name,
        remediation: remediation.into(),
    }
    .into()
}

/// Storage read by [`WeightPreflight::fetch`], in fetch order.
pub const PREFLIGHT_STORAGE: [rtao_runtime::StorageSurface; 17] = [
    st::UIDS.surface(),
    st::LAST_UPDATE.surface(),
    st::WEIGHTS_SET_RATE_LIMIT.surface(),
    st::COMMIT_REVEAL_WEIGHTS_ENABLED.surface(),
    st::COMMIT_REVEAL_WEIGHTS_VERSION.surface(),
    st::REVEAL_PERIOD_EPOCHS.surface(),
    st::WEIGHTS_VERSION_KEY.surface(),
    st::MIN_ALLOWED_WEIGHTS.surface(),
    st::MAX_WEIGHTS_LIMIT.surface(),
    st::SUBNETWORK_N.surface(),
    st::SUBNET_EPOCH_CONSENSUS.surface(),
    st::VALIDATOR_PERMIT.surface(),
    st::TEMPO.surface(),
    st::LAST_EPOCH_BLOCK.surface(),
    st::PENDING_EPOCH_AT.surface(),
    st::SUBNET_EPOCH_INDEX.surface(),
    st::BLOCKS_SINCE_LAST_STEP.surface(),
];

/// Other storage the validator run path reads with typed, shape-checked reads.
///
/// Mechanism and owner state for each scheduling pass, timelocked commit
/// tracking and the drand round used by the reveal watcher. Together with
/// [`PREFLIGHT_STORAGE`] this covers every typed storage read whose failure
/// stops the run loop; the chain-clock timestamp read only warns and retries.
/// When an entry's compiled shape no longer matches, these reads and the
/// preflight's resolve it by name against live metadata and decode the same
/// values, or refuse that entry alone when its meaning changed (D-089).
pub const RUN_PATH_STORAGE: [rtao_runtime::StorageSurface; 6] = [
    st::MECHANISM_COUNT_CURRENT.surface(),
    st::ACTIVITY_CUTOFF_FACTOR_MILLI.surface(),
    st::STAKE_THRESHOLD.surface(),
    st::SUBNET_OWNER_HOTKEY.surface(),
    st::TIMELOCKED_WEIGHT_COMMITS.surface(),
    rtao_runtime::storage::drand::LAST_STORED_ROUND.surface(),
];

/// Constants the validator run path reads with typed, shape-checked reads.
pub const RUN_PATH_CONSTANTS: [rtao_runtime::ConstantSurface; 1] =
    [rtao_runtime::constants::aura::SLOT_DURATION.surface()];

/// Runtime API read by the validator's selective metagraph sync and reveal
/// watcher.
///
/// Its output decode falls back to a dynamic decode when the typed
/// projection changes, so only absence, changed inputs, an API version below
/// two or degraded metadata stop the run loop.
pub const METAGRAPH_API: rtao_runtime::ApiSurface =
    rtao_runtime::runtime_apis::subnet_info_runtime_api::GET_SELECTIVE_MECHAGRAPH.surface();

/// Live-surface compatibility of what a validator actually uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidatorSurface {
    /// Validator dependencies this build cannot use under the live runtime,
    /// as `descriptor: detail` lines.
    pub incompatible: Vec<String>,
    /// Drift that does not stop the validator run loop: breaking changes in
    /// items outside the validator-read storage, constants, APIs and weight
    /// calls, and non-refusing changes (such as a metagraph projection change
    /// with a dynamic fallback) in validator-read items. Diagnostics report
    /// these as warnings.
    pub warnings: Vec<rtao_runtime::SurfaceChange>,
}

impl ValidatorSurface {
    /// Check what the validator run path uses against the live runtime.
    ///
    /// The result fails only where the run loop would fail or report itself
    /// not ready:
    /// - a weight call's shape changed and name-resolved composition refuses
    ///   it too (the readiness flag); a call only composed by name is a
    ///   warning (D-088);
    /// - validator-read storage or constants are absent, or changed shape in
    ///   a way the name-resolved read refuses too; a read that resolves by
    ///   name is a warning (D-089), and an entry whose absence is meaningful
    ///   fails when it is no longer `Optional`;
    /// - the selective metagraph API is absent, its inputs changed, its API
    ///   version is below two, or metadata is degraded;
    /// - breaking signing drift (V15 metadata level, extrinsic version,
    ///   signed extensions).
    ///
    /// Everything else is collected in [`Self::warnings`].
    pub fn check(runtime: &rtao_runtime::RuntimeHandle) -> Self {
        Self::check_with_encoder(runtime, false)
    }

    /// [`Self::check`] for a validator that installs a [`WeightCallEncoder`]
    /// when `encoder` is true: a changed weight call is then reported as
    /// composed through the encoder, which the scheduler prefers over name
    /// resolution, as a warning instead of a failure.
    pub fn check_with_encoder(runtime: &rtao_runtime::RuntimeHandle, encoder: bool) -> Self {
        use rtao_runtime::{SurfaceClass, SurfaceKind};
        let report = runtime.surface_report();
        let details = |descriptor: &str| {
            let detail = report
                .changes
                .iter()
                .filter(|change| change.descriptor == descriptor)
                .map(|change| change.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            if detail.is_empty() {
                "not verified by this rtao build".to_owned()
            } else {
                detail
            }
        };
        let mut surface = Self::default();
        let mut failed = std::collections::BTreeSet::new();
        let fail = |surface: &mut Self, id: String, detail: String| {
            surface.incompatible.push(format!("{id}: {detail}"));
            id
        };
        let mut composed = std::collections::BTreeSet::new();
        for call in crate::runtime::WEIGHT_CALLS {
            let id = crate::runtime::call_identity(call);
            let commit = call.name == compose::compiled(true).name;
            // The same order the scheduler selects composers in.
            let composer = match compose::route(runtime, commit, encoder) {
                Ok(compose::Route::Compiled) => continue,
                Ok(compose::Route::Application) => {
                    Ok("composes through the application call encoder")
                }
                Ok(compose::Route::Resolved) => Ok("composes by name against live metadata"),
                Err(error) => Err(error),
            };
            match composer {
                Ok(how) => {
                    surface.warnings.push(rtao_runtime::SurfaceChange {
                        descriptor: id.clone(),
                        kind: SurfaceKind::Call,
                        class: SurfaceClass::Behavioural,
                        detail: format!("weight call {how}; {}", details(&id)),
                    });
                    composed.insert(id);
                }
                Err(error) => {
                    let detail = format!(
                        "weight submission pauses; {}; name-resolved composition refused: {error}",
                        details(&id)
                    );
                    failed.insert(fail(&mut surface, id, detail));
                }
            }
        }
        let mut used = crate::runtime::WEIGHT_CALLS
            .iter()
            .map(|call| crate::runtime::call_identity(call))
            .collect::<Vec<_>>();
        let typed = PREFLIGHT_STORAGE
            .iter()
            .chain(&RUN_PATH_STORAGE)
            .map(|storage| format!("storage:{}.{}", storage.pallet, storage.name))
            .chain(
                RUN_PATH_CONSTANTS
                    .iter()
                    .map(|constant| format!("constant:{}.{}", constant.pallet, constant.name)),
            )
            .collect::<Vec<_>>();
        // Storage and constants the scheduler reads by name when their
        // compiled shape changed (D-089).
        let resolved = |surface: &mut Self, id: &str, how: &str| {
            surface.warnings.push(rtao_runtime::SurfaceChange {
                descriptor: id.to_owned(),
                kind: if id.starts_with("constant:") {
                    SurfaceKind::Constant
                } else {
                    SurfaceKind::Storage
                },
                class: SurfaceClass::Behavioural,
                detail: format!(
                    "{how} resolves by name against live metadata; {}",
                    details(id)
                ),
            });
        };
        for storage in PREFLIGHT_STORAGE.iter().chain(&RUN_PATH_STORAGE) {
            let id = format!("storage:{}.{}", storage.pallet, storage.name);
            match reads::route(runtime, storage) {
                Ok(reads::Route::Compiled) => {}
                Ok(reads::Route::Resolved) => {
                    resolved(&mut surface, &id, "storage read");
                    composed.insert(id);
                }
                Err(error) => {
                    let detail = if report.shape_matches(&id) {
                        error.to_string()
                    } else {
                        format!("{}; {error}", details(&id))
                    };
                    failed.insert(fail(&mut surface, id, detail));
                }
            }
        }
        for constant in &RUN_PATH_CONSTANTS {
            let id = format!("constant:{}.{}", constant.pallet, constant.name);
            if report.shape_matches(&id) {
                continue;
            }
            match rtao_runtime::check_constant(
                runtime,
                constant.pallet,
                constant.name,
                &reads::SLOT_DURATION,
            ) {
                Ok(()) => {
                    resolved(&mut surface, &id, "constant read");
                    composed.insert(id);
                }
                Err(error) => {
                    let detail = format!("{}; name-resolved read refused: {error}", details(&id));
                    failed.insert(fail(&mut surface, id, detail));
                }
            }
        }
        used.extend(typed);
        let api = format!("api:{}.{}", METAGRAPH_API.api, METAGRAPH_API.name);
        if let Err(detail) = metagraph_api_usable(runtime) {
            failed.insert(fail(&mut surface, api.clone(), detail));
        }
        used.push(api);
        for change in &report.changes {
            if failed.contains(&change.descriptor) || composed.contains(&change.descriptor) {
                continue;
            }
            if change.class == SurfaceClass::Breaking
                && matches!(
                    change.kind,
                    SurfaceKind::Metadata | SurfaceKind::Extrinsic | SurfaceKind::Extension
                )
            {
                failed.insert(fail(
                    &mut surface,
                    change.descriptor.clone(),
                    change.detail.clone(),
                ));
            } else if change.class == SurfaceClass::Breaking || used.contains(&change.descriptor) {
                surface.warnings.push(change.clone());
            }
        }
        surface
    }

    /// Whether the validator can operate under the checked runtime.
    pub fn is_compatible(&self) -> bool {
        self.incompatible.is_empty()
    }
}

/// Whether name-resolved composition accepts this weight call (D-088).
pub(crate) fn weight_call_resolves(
    runtime: &rtao_runtime::RuntimeHandle,
    call: &rtao_runtime::CallDescriptor,
) -> bool {
    compose::check_resolved(runtime, call.name == compose::compiled(true).name).is_ok()
}

/// Whether the selective metagraph call the validator makes can be encoded.
///
/// Mirrors the metagraph fetch: degraded metadata, an absent method, an API
/// version below two, or inputs that changed name, order or encoding refuse.
fn metagraph_api_usable(runtime: &rtao_runtime::RuntimeHandle) -> Result<(), String> {
    use rtao_core::ops::runtime_api;
    if runtime.metadata_level() != rtao_runtime::MetadataLevel::V15 {
        return Err("runtime API signatures require V15 metadata".into());
    }
    let have = runtime.runtime_api_version(METAGRAPH_API.api).unwrap_or(0);
    if have < 2 {
        return Err(format!(
            "the metagraph fetch requires API version 2, found {have}"
        ));
    }
    let method =
        runtime_api::runtime_api_method(runtime.core(), METAGRAPH_API.api, METAGRAPH_API.name)
            .map_err(|_| "runtime API method is absent".to_owned())?;
    if method.inputs.len() != METAGRAPH_API.inputs.len()
        || METAGRAPH_API
            .inputs
            .iter()
            .zip(&method.inputs)
            .any(|(expected, actual)| expected.name != actual.name)
    {
        return Err("runtime API input names or order changed".into());
    }
    let sample = [
        Value::Int(1),
        Value::Int(0),
        Value::List(vec![Value::Int(0)]),
    ];
    runtime_api::encode_params(runtime.core(), METAGRAPH_API.api, method, &sample)
        .map(|_| ())
        .map_err(|error| format!("runtime API inputs no longer encode: {error}"))
}

/// The seventeen storage fields used by one mechanism's preflight.
#[derive(Clone, Debug)]
pub struct WeightPreflight {
    /// Block hash and best height used for every field and the predictor.
    pub at: rtao_client::PinnedBlock,
    /// Current UID, absent when the hotkey is unregistered.
    pub uid: Option<Uid>,
    /// Per-UID last updates for this mechanism.
    pub last_updates: Vec<u64>,
    /// Minimum interval between weight submissions.
    pub rate_limit: u64,
    /// Select the timelocked path rather than direct mechanism weights.
    pub commit_reveal: bool,
    /// Version tag the runtime requires on each timelocked commit.
    pub commit_reveal_version: u16,
    /// `:code` storage hash at [`Self::at`], read only while commit-reveal is
    /// enabled and the stored tag differs from [`NATIVE_CR_PAYLOAD_VERSION`].
    pub runtime_code: Option<rtao_primitives::H256>,
    /// Number of epochs between commitment and reveal.
    pub reveal_period: u64,
    /// Runtime's forced-upgrade key.
    pub required_version_key: u64,
    /// Required count before population capping.
    pub min_allowed_weights: u16,
    /// Maximum relative share, represented over 65535.
    pub max_weights_limit: u16,
    /// Current registered population.
    pub population: u16,
    /// Whether consensus is Null rather than Yuma.
    pub null_consensus: bool,
    /// Current validator-permit vector.
    pub permits: Vec<bool>,
    /// Coherent schedule at the pinned best head.
    pub schedule: EpochScheduleState,
}

impl WeightPreflight {
    /// The [`PREFLIGHT_STORAGE`] reads for one mechanism and hotkey.
    fn reads(
        netuid: NetUid,
        mech: MechId,
        hotkey: AccountId,
    ) -> anyhow::Result<Vec<reads::DynRead>> {
        use reads::KeyDatum;
        let index = NetUidStorageIndex::new(netuid, mech)?;
        Ok(PREFLIGHT_STORAGE
            .into_iter()
            .enumerate()
            .map(|(position, surface)| {
                let (keys, data) = match position {
                    0 => (
                        vec![netuid.into_core_value(), hotkey.into_core_value()],
                        vec![netuid.key_datum(), hotkey.key_datum()],
                    ),
                    1 => (vec![index.into_core_value()], vec![index.key_datum()]),
                    4 => (Vec::new(), Vec::new()),
                    _ => (vec![netuid.into_core_value()], vec![netuid.key_datum()]),
                };
                (surface, keys, data)
            })
            .collect())
    }

    /// Fetch all seventeen fields using one `state_queryStorageAt` at `snapshot`.
    pub async fn fetch(
        snapshot: &Snapshot,
        netuid: NetUid,
        mech: MechId,
        hotkey: AccountId,
    ) -> anyhow::Result<Self> {
        let plan = Self::reads(netuid, mech, hotkey)?;
        let values = reads::fetch_dyn(snapshot, plan).await?;
        anyhow::ensure!(
            values.len() == 17,
            "weight preflight omitted storage fields"
        );
        let integer = |index, name| number(&values[index], name);
        let uid = if matches!(values[0].as_core(), Value::Null) {
            None
        } else {
            Some(Uid(u16::try_from(integer(0, "Uids")?)?))
        };
        let commit_reveal = boolean(&values[3], "CommitRevealWeightsEnabled")?;
        let commit_reveal_version = u16::try_from(integer(4, "CommitRevealWeightsVersion")?)?;
        // The default tag needs no code identity, so it costs no extra read.
        let runtime_code = if commit_reveal && commit_reveal_version != NATIVE_CR_PAYLOAD_VERSION {
            Some(runtime_code_hash(snapshot.client(), snapshot.block().hash).await?)
        } else {
            None
        };
        Ok(Self {
            at: snapshot.block().clone(),
            uid,
            last_updates: integers(&values[1], "LastUpdate")?,
            rate_limit: integer(2, "WeightsSetRateLimit")?,
            commit_reveal,
            commit_reveal_version,
            runtime_code,
            reveal_period: integer(5, "RevealPeriodEpochs")?,
            required_version_key: integer(6, "WeightsVersionKey")?,
            min_allowed_weights: u16::try_from(integer(7, "MinAllowedWeights")?)?,
            max_weights_limit: u16::try_from(integer(8, "MaxWeightsLimit")?)?,
            population: u16::try_from(integer(9, "SubnetworkN")?)?,
            null_consensus: consensus_null(&values[10])?,
            permits: booleans(&values[11], "ValidatorPermit")?,
            schedule: EpochScheduleState {
                tempo: u16::try_from(integer(12, "Tempo")?)?,
                last_epoch_block: integer(13, "LastEpochBlock")?,
                pending_epoch_at: integer(14, "PendingEpochAt")?,
                subnet_epoch_index: integer(15, "SubnetEpochIndex")?,
                blocks_since_last_step: integer(16, "BlocksSinceLastStep")?,
                current_block: snapshot.block().number,
            },
        })
    }

    /// Check certain registration, Null permit and client upgrade refusals.
    ///
    /// This check is strict about the commit-reveal tag: it refuses any stored
    /// version other than [`NATIVE_CR_PAYLOAD_VERSION`]. The scheduler instead
    /// follows a stored tag when the live runtime code is known to reveal the
    /// native payload (D-086).
    pub fn check(&self, version_key: u64) -> anyhow::Result<Uid> {
        let uid = self.check_except_cr(version_key)?;
        if self.commit_reveal && self.commit_reveal_version != NATIVE_CR_PAYLOAD_VERSION {
            return Err(self.cr_refusal(false).into());
        }
        Ok(uid)
    }
    /// Refusal for a stored commit-reveal tag this build does not follow here.
    /// `refuted` reports that a commitment tagged with it under the live code
    /// missed its reveal with no other explanation.
    fn cr_refusal(&self, refuted: bool) -> WeightRefusal {
        let code = self.runtime_code.map_or_else(
            || "unread".to_owned(),
            |code| format!("0x{}", hex::encode(code.0)),
        );
        let default = metadata_cr_default(&self.at.runtime)
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string());
        WeightRefusal {
            code: rtao_catalog::ErrorCode::InvalidArgument,
            name: "IncorrectCommitRevealVersion",
            remediation: if refuted {
                format!(
                    "the chain requires commit-reveal version {tag}; this rtao build encodes the version {NATIVE_CR_PAYLOAD_VERSION} payload, and a commitment this validator tagged {tag} under runtime code {code} missed its reveal with no drand, epoch or chain-check explanation (metadata default {default}); timelocked commits pause for this mechanism while other duties continue; they resume once the chain returns to version {NATIVE_CR_PAYLOAD_VERSION} or an outstanding commitment reveals under this code and version; otherwise upgrade rtao to a build that supports this runtime's payload",
                    tag = self.commit_reveal_version
                )
            } else {
                format!(
                    "the chain requires commit-reveal version {tag}; this rtao build encodes the version {NATIVE_CR_PAYLOAD_VERSION} payload and has not seen runtime code {code} reveal it while version {tag} was stored (metadata default {default}); timelocked commits pause for this mechanism while other duties continue; they resume once the chain returns to version {NATIVE_CR_PAYLOAD_VERSION} or an outstanding commitment reveals under this code and version; otherwise upgrade rtao to a build that reviewed this runtime",
                    tag = self.commit_reveal_version
                )
            },
        }
    }
    /// Registration, Null permit and weight version-key checks, without the
    /// commit-reveal tag.
    fn check_except_cr(&self, version_key: u64) -> anyhow::Result<Uid> {
        let uid = self.uid.ok_or_else(|| {
            refuse(
                rtao_catalog::ErrorCode::NotRegistered,
                "HotKeyNotRegisteredInSubNet",
                "register this hotkey in the configured subnet",
            )
        })?;
        if self.null_consensus
            && !self
                .permits
                .get(usize::from(uid.0))
                .copied()
                .unwrap_or(false)
        {
            return Err(refuse(
                rtao_catalog::ErrorCode::NotAuthorized,
                "NeuronNoValidatorPermit",
                "Null consensus requires this hotkey to hold the validator permit",
            ));
        }
        if version_key < self.required_version_key {
            return Err(refuse(
                rtao_catalog::ErrorCode::InvalidArgument,
                "IncorrectWeightVersionKey",
                format!(
                    "the chain requires weight version {}; configure a scoring implementation that supports this version",
                    self.required_version_key
                ),
            ));
        }
        Ok(uid)
    }
    fn last_update(&self, uid: Uid) -> u64 {
        self.last_updates
            .get(usize::from(uid.0))
            .copied()
            .unwrap_or(0)
    }
    fn activity_alarm_cutoff(&self, uid: Uid, activity_factor: u32) -> Option<u64> {
        let cutoff =
            u64::from(activity_factor).saturating_mul(u64::from(self.schedule.tempo)) / 1000;
        let cutoff = cutoff.max(1);
        (self.at.number.saturating_sub(self.last_update(uid)) >= cutoff.saturating_mul(4) / 5)
            .then_some(cutoff)
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Slot {
    last_attempt_epoch: Option<u64>,
    last_attempt_block: Option<u64>,
    uncertain: bool,
    /// After attempts released because they signed nothing (D-095): the
    /// first block the next attempt may start and how many were released
    /// in a row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unsigned_retry: Option<(u64, u32)>,
}

/// Advisory conditions currently reported for one mechanism. Each warning is
/// logged when its condition starts and again only after it clears, so a
/// persistent condition does not repeat on every block; metrics still count
/// every occurrence.
#[derive(Default)]
struct Advisories {
    permit_absent: bool,
    stake_below_threshold: bool,
    activity_alarm: bool,
    cr_version_paused: bool,
    cr_version_followed: Option<(u16, Option<rtao_primitives::H256>)>,
    /// Composer and runtime of the last call composed without the compiled
    /// descriptor, so each switch is logged once (D-088).
    composed_with: Option<(&'static str, rtao_runtime::RuntimeStamp)>,
}

/// Metadata default of `CommitRevealWeightsVersion`, a diagnostic hint only:
/// root can store any value, and only runtime code fixes the payload format.
fn metadata_cr_default(runtime: &rtao_runtime::RuntimeHandle) -> Option<u16> {
    let entry = runtime.storage_entry("SubtensorModule", "CommitRevealWeightsVersion")?;
    let bytes = <[u8; 2]>::try_from(entry.default_bytes.as_slice()).ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// Record whether an advisory holds and report whether it just started.
fn advisory_started(reported: &mut bool, holds: bool) -> bool {
    let started = holds && !*reported;
    *reported = holds;
    started
}

/// One scheduler owns cadence slots and outstanding commits for a validator.
pub struct WeightScheduler {
    policy: WeightPolicy,
    clock: Arc<dyn Clock>,
    clock_guard: ClockGuard,
    slots: BTreeMap<MechId, Slot>,
    attempts: BTreeMap<MechId, WeightAttempt>,
    advisories: BTreeMap<MechId, Advisories>,
    reveals: RevealWatcher,
    durable: Option<durable::Durable>,
    semantic_policy: Option<String>,
    semantic_guard: Option<Arc<dyn rtao_client::journal::SemanticGuard>>,
    shield_protocol: Option<Arc<dyn rtao_client::journal::ShieldProtocolGuard>>,
    shield_period: Option<rtao_client::ShieldPeriod>,
    call_encoder: Option<Arc<dyn WeightCallEncoder>>,
}
impl WeightScheduler {
    /// Construct a scheduler with explicit wall time and a best-head skew guard.
    pub fn new(
        policy: WeightPolicy,
        clock: Arc<dyn Clock>,
        clock_guard: ClockGuard,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            policy,
            clock,
            clock_guard,
            slots: BTreeMap::new(),
            attempts: BTreeMap::new(),
            advisories: BTreeMap::new(),
            reveals: RevealWatcher::default(),
            durable: None,
            semantic_policy: None,
            semantic_guard: None,
            shield_protocol: None,
            shield_period: None,
            call_encoder: None,
        })
    }
    /// Install an application weight call encoder (D-088).
    ///
    /// It composes a mechanism's call only while that call's compiled
    /// descriptor no longer matches the live runtime, before name-resolved
    /// composition. Its bytes are imported and validated before signing; see
    /// [`WeightCallEncoder`]. Each durable attempt it composes records its
    /// [`WeightCallEncoder::contract`] in the attempt material, outside the
    /// operation's identity. Retained work with such a signed attempt is
    /// rebuilt only while an encoder with the same contract is installed, so
    /// reinstall it on restart.
    pub fn with_call_encoder(mut self, encoder: Arc<dyn WeightCallEncoder>) -> Self {
        self.call_encoder = Some(encoder);
        self
    }
    /// Explicitly request shielded weight submission through the same public
    /// executor/journal. Ordinary scheduling remains the default. Reinstall on
    /// restart; the journal retains the original per-operation shield policy.
    /// Runtime code is qualified by `NativeShieldProtocol` unless
    /// [`Self::with_shield_protocol`] installs another adapter; finalized
    /// waits, native author custody and the semantic guard still apply.
    pub fn with_shielded_submission(mut self, period: rtao_client::ShieldPeriod) -> Self {
        self.shield_period = Some(period);
        self
    }
    /// Borrow retained reveal outcomes and outstanding commitment state.
    pub fn reveals(&self) -> &RevealWatcher {
        &self.reveals
    }
    /// Replace operator policy while preserving uncertainty and reveal tracking.
    pub fn update_policy(&mut self, policy: WeightPolicy) {
        self.policy = policy;
    }
    /// Inspect the latest mechanism outcome and any actionable refusal.
    pub fn last_attempt(&self, mechanism: MechId) -> Option<&WeightAttempt> {
        self.attempts.get(&mechanism)
    }
    /// Inspect scheduling using a fresh coherent preflight, without submitting.
    /// Recompute after every relevant parameter change; this is not a future
    /// chain guarantee or a replacement for runtime/policy/semantic checks.
    pub fn next_action(
        &self,
        mechanism: MechId,
        hotkey: AccountId,
        state: &WeightPreflight,
    ) -> anyhow::Result<WeightOpportunity> {
        if let Some(d) = &self.durable {
            anyhow::ensure!(hotkey == d.account, "scheduler account mismatch");
            if d.journal.snapshot()?.control.submissions_paused {
                return Ok(WeightOpportunity::Paused);
            }
        }
        let pending = self.pending_work(mechanism)?;
        if self.reveals.pending_for(mechanism) >= 10
            || self.slots.get(&mechanism).is_some_and(|s| s.uncertain)
            || pending.as_ref().is_some_and(|(binding, _)| {
                match self.shield_period {
                    Some(period) => binding.can_prepare_shield(period),
                    None => binding.can_dispatch(),
                }
                .is_err()
            })
        {
            return Ok(WeightOpportunity::Reconcile);
        }
        let version = pending
            .as_ref()
            .map_or(self.policy.version_key.value(), |(_, w)| w.version_key);
        let uid = state.check_except_cr(version)?;
        if self.commit_reveal_tag(state).is_err() {
            return Ok(WeightOpportunity::Paused);
        }
        let cadence = self
            .policy
            .cadence
            .unwrap_or_else(|| default_cadence(hotkey, state.schedule.tempo, state.commit_reveal));
        let empty = Slot::default();
        Ok(cadence_opportunity(
            cadence,
            self.slots.get(&mechanism).unwrap_or(&empty),
            state,
            uid,
        ))
    }
    /// Version tag this scheduler puts on a timelocked commit under `state`.
    ///
    /// The chain only requires the tag to equal `CommitRevealWeightsVersion`;
    /// the runtime code alone decides which payload a reveal decodes (D-086).
    /// The stored tag is followed when it is [`NATIVE_CR_PAYLOAD_VERSION`], or
    /// when the live `:code` is listed in [`REVIEWED_CR_RUNTIME_CODE`] or one of
    /// this validator's commitments revealed under it while the same tag was
    /// stored. A later unexplained miss of a commitment tagged with it under
    /// that code refutes either reason. Otherwise the refusal explains why
    /// timelocked commits pause; the scheduler retries every block.
    pub fn commit_reveal_tag(&self, state: &WeightPreflight) -> Result<u16, WeightRefusal> {
        let tag = state.commit_reveal_version;
        if !state.commit_reveal || tag == NATIVE_CR_PAYLOAD_VERSION {
            return Ok(NATIVE_CR_PAYLOAD_VERSION);
        }
        let Some(code) = state.runtime_code else {
            return Err(state.cr_refusal(false));
        };
        if self.reveals.native_reveal_refuted(code, tag) {
            return Err(state.cr_refusal(true));
        }
        if reviewed_cr_runtime(code).is_some() || self.reveals.reveals_native_payload(code, tag) {
            Ok(tag)
        } else {
            Err(state.cr_refusal(false))
        }
    }

    /// Select the commit tag for one attempt, or record a non-fatal pause.
    ///
    /// A pause is an operator-visible refusal, logged once while it lasts. It
    /// never counts as a scheduler error; other mechanisms and duties continue.
    fn follow_cr_tag(
        &mut self,
        metrics: &crate::Metrics,
        mech: MechId,
        state: &WeightPreflight,
    ) -> Option<u16> {
        let selected = self.commit_reveal_tag(state);
        let gauge = format!("rtao_weights_cr_version_paused{{mech=\"{}\"}}", mech.0);
        let advisories = self.advisories.entry(mech).or_default();
        match selected {
            Ok(tag) => {
                let resumed = std::mem::take(&mut advisories.cr_version_paused);
                // Direct-row mechanisms publish the gauge only to clear a pause.
                if state.commit_reveal || resumed {
                    metrics.gauge(&gauge, 0.0);
                }
                if resumed {
                    tracing::info!(
                        mechanism = mech.0,
                        commit_reveal_version = tag,
                        "timelocked weight commits resumed"
                    );
                }
                let followed =
                    (tag != NATIVE_CR_PAYLOAD_VERSION).then_some((tag, state.runtime_code));
                if followed.is_some() && advisories.cr_version_followed != followed {
                    tracing::info!(
                        mechanism = mech.0,
                        commit_reveal_version = tag,
                        runtime_code = ?state.runtime_code,
                        reviewed_spec = ?state.runtime_code.and_then(reviewed_cr_runtime),
                        "following the chain's commit-reveal version under runtime code known to reveal the version {NATIVE_CR_PAYLOAD_VERSION} payload"
                    );
                }
                advisories.cr_version_followed = followed;
                Some(tag)
            }
            Err(refusal) => {
                metrics.increment("rtao_weights_cr_version_mismatch_total");
                metrics.gauge(&gauge, 1.0);
                metrics.update_readiness(|ready| ready.last_weight_succeeded = false);
                if advisory_started(&mut advisories.cr_version_paused, true) {
                    tracing::warn!(
                        mechanism = mech.0,
                        commit_reveal_version = state.commit_reveal_version,
                        error = %refusal,
                        "timelocked weight commits paused"
                    );
                }
                advisories.cr_version_followed = None;
                self.attempts.insert(
                    mech,
                    WeightAttempt {
                        mechanism: mech,
                        best_block: state.at.number,
                        outcome: WeightAttemptOutcome::Refused(refusal),
                    },
                );
                None
            }
        }
    }

    /// Encoder this mechanism's work may use, or the contract retained work
    /// requires when its encoder is not installed (D-088).
    ///
    /// Only work with a signed attempt the encoder composed requires it: its
    /// contract is recorded in that attempt's material. Other work, including
    /// work queued while an encoder was installed but composed from the
    /// compiled descriptor, uses whatever composers are installed now.
    fn work_encoder(
        &self,
        binding: Option<&rtao_client::journal::JournalBinding>,
    ) -> anyhow::Result<Result<Option<Arc<dyn WeightCallEncoder>>, String>> {
        let required = match binding {
            Some(binding) => durable::signed_encoder_contract(&binding.record()?)?,
            None => None,
        };
        Ok(match (required, &self.call_encoder) {
            (None, encoder) => Ok(encoder.clone()),
            (Some(contract), Some(encoder)) if encoder.contract() == contract => {
                Ok(Some(encoder.clone()))
            }
            (Some(contract), _) => Err(contract),
        })
    }

    /// Select how this attempt composes its call, or pause the mechanism when
    /// neither the compiled descriptor, an installed encoder nor name-resolved
    /// composition can produce it.
    fn select_composer(
        &mut self,
        metrics: &crate::Metrics,
        mech: MechId,
        state: &WeightPreflight,
        encoder: Option<&Arc<dyn WeightCallEncoder>>,
    ) -> Option<compose::Composer> {
        let runtime = &state.at.runtime;
        match compose::select(runtime, state.commit_reveal, encoder) {
            Ok(composer) => Some(composer),
            Err(resolved) => {
                let call = compose::compiled(state.commit_reveal);
                let descriptor = crate::runtime::call_identity(call);
                let difference = runtime
                    .surface_report()
                    .changes
                    .iter()
                    .filter(|change| change.descriptor == descriptor)
                    .map(|change| change.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                self.pause_surface(
                    metrics,
                    mech,
                    state.at.number,
                    descriptor,
                    format!(
                        "weight call is incompatible with this rtao build; update rtao or install a WeightCallEncoder and re-verify the runtime: {difference}; {resolved}"
                    ),
                );
                None
            }
        }
    }

    /// Compose this attempt's call, or pause the mechanism without signing
    /// when the composer refuses it.
    ///
    /// `guarded` is true only when the attempt dispatches through a journal
    /// binding that runs the installed semantic guard
    /// ([`Self::semantic_guard_runs`]); an encoder's declared calls need it.
    fn compose_call(
        &mut self,
        metrics: &crate::Metrics,
        mech: MechId,
        state: &WeightPreflight,
        composer: &compose::Composer,
        plan: &submit::WeightCallPlan,
        guarded: bool,
    ) -> Option<submit::Composed> {
        let runtime = &state.at.runtime;
        let Some(result) =
            plan.with_call(|call| compose::compose_verified(composer, runtime, call, guarded))
        else {
            return Some(submit::Composed::Compiled);
        };
        let metric = match composer {
            compose::Composer::Application(_) => "rtao_weights_encoder_composition_total",
            _ => "rtao_weights_resolved_composition_total",
        };
        match result {
            Ok(call) => {
                metrics.increment(metric);
                let current = Some((composer.label(), runtime.stamp()));
                let advisories = self.advisories.entry(mech).or_default();
                if advisories.composed_with != current {
                    advisories.composed_with = current;
                    tracing::warn!(
                        mechanism = mech.0,
                        composer = composer.label(),
                        spec_version = runtime.spec_version(),
                        "the compiled weight call no longer matches the live runtime; composing it without the compiled descriptor"
                    );
                }
                Some(submit::Composed::Verified(submit::VerifiedCall::new(
                    call,
                    composer.clone(),
                    guarded,
                )))
            }
            Err(detail) => {
                let call = compose::compiled(plan.commit.is_some());
                self.pause_surface(
                    metrics,
                    mech,
                    state.at.number,
                    crate::runtime::call_identity(call),
                    detail,
                );
                None
            }
        }
    }

    fn pause_surface(
        &mut self,
        metrics: &crate::Metrics,
        mech: MechId,
        best_block: u64,
        descriptor: String,
        detail: String,
    ) {
        self.attempts.insert(
            mech,
            WeightAttempt {
                mechanism: mech,
                best_block,
                outcome: WeightAttemptOutcome::PausedSurface { descriptor, detail },
            },
        );
        metrics.increment("rtao_weights_surface_paused_total");
        metrics.update_readiness(|ready| {
            ready.runtime_verified = false;
            ready.last_weight_succeeded = false;
        });
    }

    /// Clear a legacy in-memory uncertainty block after external reconciliation.
    ///
    /// The scheduler normally reconciles through the executor. An unresolved
    /// result remains blocked so the same pool provides-tag cannot be duplicated.
    /// Durable schedulers ignore this toggle and require retained chain evidence.
    pub fn resolve_uncertainty(&mut self, mechanism: MechId) {
        // A manual memory toggle cannot authorize replacement of durable bytes.
        if self.durable.is_some() {
            return;
        }
        if let Some(slot) = self.slots.get_mut(&mechanism) {
            slot.uncertain = false;
        }
    }

    /// Observe a finalized head, then prepare due proposals at one fresh best head.
    ///
    /// Every proposal is prepared under [`OwnerRowPolicy::Hold`]; use
    /// [`Self::on_finalized_with`] to choose the policy per proposal.
    ///
    /// An application may drive this from its own loop, without
    /// `NeuronRuntime`. Before each timelocked commit the scheduler reads
    /// `Timestamp.Now` at the best block the commit's preflight is pinned to
    /// and observes it on its clock and [`ClockGuard`] (D-094): a
    /// [`crate::ChainClock::unanchored`] clock is anchored from it, and a
    /// [`crate::SystemClock`] gets its skew estimate. The clock only moves
    /// forward, so a fresher anchor, such as the runtime's clock worker
    /// keeps, is never replaced by an older block's timestamp.
    ///
    /// A mechanism's cadence slot is reserved before its weights are
    /// dispatched. A dispatch that fails before anything was signed, such as
    /// an admission timeout while reads queue on a paced endpoint, gives the
    /// slot back: the same epoch may still commit, after one block, then two,
    /// doubling to 32 blocks while such failures repeat. A failure after the
    /// hotkey was asked to sign, or with a new signed attempt in the journal,
    /// keeps the slot (D-095).
    pub async fn on_finalized(
        &mut self,
        cx: &NeuronCx,
        finalized: &BlockHeader,
        mut proposals: impl FnMut(MechId) -> WeightsProposal,
    ) -> anyhow::Result<()> {
        self.on_finalized_with(cx, finalized, |mech| {
            (proposals(mech), OwnerRowPolicy::Hold)
        })
        .await
    }

    /// [`Self::on_finalized`] whose callback also returns the
    /// [`OwnerRowPolicy`] for its proposal (D-093).
    ///
    /// The policy is ignored for [`WeightsProposal::HoldPrevious`] and
    /// [`WeightsProposal::Skip`]. A durable scheduler journals it with the
    /// operation the proposal creates. Retained journaled work keeps the policy
    /// it was queued with: the callback is not consulted while such work is
    /// pending.
    pub async fn on_finalized_with(
        &mut self,
        cx: &NeuronCx,
        finalized: &BlockHeader,
        mut proposals: impl FnMut(MechId) -> (WeightsProposal, OwnerRowPolicy),
    ) -> anyhow::Result<()> {
        if let Some(d) = &self.durable {
            anyhow::ensure!(
                cx.netuid == d.netuid && cx.hotkey.hotkey_address()? == d.account,
                "scheduler context differs from durable identity"
            );
        }
        Box::pin(self.recover_finalized(&cx.client, finalized, &cx.metrics)).await?;
        if !self.reveals.caught_up(finalized.number) {
            return Ok(());
        }
        if let Some(durable) = &self.durable
            && durable.journal.snapshot()?.control.submissions_paused
        {
            return Ok(());
        }
        if cx.shutdown.is_cancelled() {
            return Ok(());
        }
        let snapshot = cx.client.at(BlockRef::Best).await?;
        let pinned = crate::clock::Pinned::now();
        let hotkey = cx.hotkey.hotkey_address()?;
        let auxiliary = reads::fetch(
            &snapshot,
            (
                st::MECHANISM_COUNT_CURRENT.at((cx.netuid,)),
                st::ACTIVITY_CUTOFF_FACTOR_MILLI.at((cx.netuid,)),
                st::STAKE_THRESHOLD.at(()),
                st::SUBNET_OWNER_HOTKEY.at((cx.netuid,)),
            ),
        )
        .await?;
        let mechanisms = auxiliary.0.0;
        anyhow::ensure!(
            (1..=16).contains(&mechanisms),
            "runtime mechanism count is outside 1..=16"
        );
        let owner = auxiliary.3.raw();
        for mechanism in 0..mechanisms {
            let mech = MechId(mechanism);
            let preflight = WeightPreflight::fetch(&snapshot, cx.netuid, mech, hotkey).await?;
            let best_block = preflight.at.number;
            // Keep the callback future and native debug polling stack bounded
            // across the preparation/signing/recovery state machines.
            if let Err(error) = Box::pin(self.attempt(
                cx,
                hotkey,
                *owner,
                auxiliary.1,
                auxiliary.2,
                mechanisms,
                mech,
                preflight,
                pinned,
                &mut proposals,
            ))
            .await
            {
                let outcome = if let Some(refusal) = error.downcast_ref::<WeightRefusal>() {
                    WeightAttemptOutcome::Refused(refusal.clone())
                } else if submit::uncertain(&error)
                    // A dispatch that provably signed nothing returned its
                    // slot without marking it uncertain (D-095). It retries
                    // on its own, so it reports a failure, not unresolved work.
                    && self.slots.get(&mech).is_some_and(|slot| slot.uncertain)
                {
                    WeightAttemptOutcome::Unresolved(error.to_string())
                } else {
                    WeightAttemptOutcome::Failed(error.to_string())
                };
                self.attempts.insert(
                    mech,
                    WeightAttempt {
                        mechanism: mech,
                        best_block,
                        outcome,
                    },
                );
                cx.metrics.increment("rtao_weights_errors_total");
                cx.metrics
                    .update_readiness(|ready| ready.last_weight_succeeded = false);
                tracing::warn!(mechanism = mech.0, error = %error, "weight attempt failed");
            }
            if cx.shutdown.is_cancelled() {
                break;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn attempt(
        &mut self,
        cx: &NeuronCx,
        hotkey: AccountId,
        owner: AccountId,
        activity_factor: u32,
        stake_threshold: u64,
        mechanisms: u8,
        mech: MechId,
        preflight: WeightPreflight,
        pinned: crate::clock::Pinned,
        proposals: &mut impl FnMut(MechId) -> (WeightsProposal, OwnerRowPolicy),
    ) -> anyhow::Result<()> {
        if self.slots.get(&mech).is_some_and(|slot| slot.uncertain) {
            return Ok(());
        }
        if self.reveals.pending_for(mech) >= 10 {
            cx.metrics.increment("rtao_weights_pending_limit_total");
            return Ok(());
        }
        self.attempts.insert(
            mech,
            WeightAttempt {
                mechanism: mech,
                best_block: preflight.at.number,
                outcome: WeightAttemptOutcome::Scheduled,
            },
        );
        let pending_work = self.pending_work(mech)?;
        let encoder = match self.work_encoder(pending_work.as_ref().map(|(binding, _)| binding))? {
            Ok(encoder) => encoder,
            Err(contract) => {
                self.attempts.insert(
                    mech,
                    WeightAttempt {
                        mechanism: mech,
                        best_block: preflight.at.number,
                        outcome: WeightAttemptOutcome::Unresolved(format!(
                            "call encoder {contract} required: it composed a signed attempt of retained work; reinstall it with WeightScheduler::with_call_encoder"
                        )),
                    },
                );
                return Ok(());
            }
        };
        let Some(composer) = self.select_composer(&cx.metrics, mech, &preflight, encoder.as_ref())
        else {
            return Ok(());
        };
        let version_key = pending_work
            .as_ref()
            .map_or(self.policy.version_key.value(), |(_, work)| {
                work.version_key
            });
        let uid = preflight.check_except_cr(version_key)?;
        let Some(cr_tag) = self.follow_cr_tag(&cx.metrics, mech, &preflight) else {
            return Ok(());
        };
        let advisories = self.advisories.entry(mech).or_default();
        let permit_absent = !preflight.null_consensus
            && !preflight
                .permits
                .get(usize::from(uid.0))
                .copied()
                .unwrap_or(false);
        if advisory_started(&mut advisories.permit_absent, permit_absent) {
            tracing::warn!(
                mechanism = mech.0,
                "Yuma validator permit is absent; chain checks remain authoritative"
            );
        }
        let graph = cx.metagraph.load();
        let stake_below_threshold = hotkey != owner
            && graph
                .by_hotkey(&hotkey)
                .and_then(|neuron| neuron.total_stake().ok())
                .is_some_and(|stake| stake.0 < stake_threshold);
        if advisory_started(&mut advisories.stake_below_threshold, stake_below_threshold) {
            tracing::warn!(
                stake_threshold,
                "last-good stake is below the chain threshold; chain checks remain authoritative"
            );
        }
        let activity_cutoff = preflight.activity_alarm_cutoff(uid, activity_factor);
        if activity_cutoff.is_some() {
            cx.metrics.increment("rtao_weights_activity_alarm_total");
        }
        if advisory_started(&mut advisories.activity_alarm, activity_cutoff.is_some())
            && let Some(cutoff) = activity_cutoff
        {
            tracing::warn!(
                mechanism = mech.0,
                cutoff_blocks = cutoff,
                "validator is approaching the activity cutoff"
            );
        }
        let slot = self.slots.entry(mech).or_default();
        if slot.uncertain {
            return Ok(());
        }
        let cadence = self.policy.cadence.unwrap_or_else(|| {
            default_cadence(hotkey, preflight.schedule.tempo, preflight.commit_reveal)
        });
        let last_submission = preflight
            .last_update(uid)
            .max(slot.last_attempt_block.unwrap_or(0));
        if last_submission != 0
            && preflight.at.number.saturating_sub(last_submission) < preflight.rate_limit
        {
            if let Some(attempt) = self.attempts.get_mut(&mech) {
                attempt.outcome = WeightAttemptOutcome::RateLimited {
                retry_after_block: last_submission.saturating_add(preflight.rate_limit),
                remediation: "wait for the mechanism's chain rate-limit slot; this is scheduled work, not a failed transaction".into(),
            };
            }
            return Ok(());
        }
        if !due(cadence, slot, &preflight, uid) {
            return Ok(());
        }
        // Retained work decides from its journaled policy, never the callback.
        let (proposal, mut owner_rows) = match &pending_work {
            Some((_, work)) => (WeightsProposal::Set(work.input.clone()), work.owner_rows),
            None => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proposals(mech)))
                .map_err(|_| anyhow::anyhow!("validator weights callback panicked"))?,
        };
        let (input, operation) = match proposal {
            WeightsProposal::Set(input) => (input, None),
            WeightsProposal::Operation {
                id,
                correlation_id,
                input,
            } => {
                anyhow::ensure!(
                    self.durable.is_some(),
                    "application operation IDs require a durable scheduler"
                );
                (input, Some((id, correlation_id)))
            }
            WeightsProposal::HoldPrevious => {
                if let Some(attempt) = self.attempts.get_mut(&mech) {
                    attempt.outcome = WeightAttemptOutcome::HoldPrevious;
                }
                cx.metrics.increment("rtao_weights_hold_total");
                return Ok(());
            }
            WeightsProposal::Skip(reason) => {
                if let Some(attempt) = self.attempts.get_mut(&mech) {
                    attempt.outcome = WeightAttemptOutcome::Skipped(reason.into());
                }
                tracing::debug!(mechanism = mech.0, reason, "weight proposal skipped");
                return Ok(());
            }
        };
        let prepared = prepare_row(input.clone(), &preflight, uid, cx.netuid, &graph);
        if self.durable.is_some() && pending_work.is_none() {
            let d = self
                .durable
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing journal"))?;
            let snapshot = d.journal.snapshot()?;
            let (id, correlation) = operation
                .unwrap_or_else(|| (format!("{}:op:{}", d.scope, snapshot.revision), None));
            // A new callback row the guard holds is not journaled: queued work
            // is re-proposed without the callback, so journaling it would hold
            // the mechanism on every later head (D-093). Nothing is signed
            // either way, and the callback decides again at the next head. An
            // ID the journal already has still goes through `begin`, which
            // reports a redelivery or an identity conflict.
            if !snapshot.operations.contains_key(&id)
                && let Ok(row) = &prepared
                && row_held(owner_rows, row, uid, hotkey, owner, &graph)
            {
                if let Some(attempt) = self.attempts.get_mut(&mech) {
                    attempt.outcome = WeightAttemptOutcome::HoldPrevious;
                }
                cx.metrics.increment("rtao_weights_hold_total");
                return Ok(());
            }
            self.enqueue_with_owner_rows(id, correlation, mech, input.clone(), owner_rows)?;
            let Some((_, queued)) = self.pending_work(mech)? else {
                // The application redelivered a completed logical operation.
                return Ok(());
            };
            // The journaled policy decides, even on the first attempt.
            owner_rows = queued.owner_rows;
        }
        let row = prepared?;
        if row_held(owner_rows, &row, uid, hotkey, owner, &graph) {
            if let Some(attempt) = self.attempts.get_mut(&mech) {
                attempt.outcome = WeightAttemptOutcome::HoldPrevious;
            }
            cx.metrics.increment("rtao_weights_hold_total");
            return Ok(());
        }
        if row_held(OwnerRowPolicy::Hold, &row, uid, hotkey, owner, &graph) {
            cx.metrics.increment("rtao_weights_owner_row_total");
            tracing::info!(
                mechanism = mech.0,
                "submitting an owner-only row under OwnerRowPolicy::Submit"
            );
        }
        let block_time = snapshot_slot_duration(&cx.client, &preflight.at).await?;
        let mut commit = None;
        if preflight.commit_reveal {
            self.anchor_clock(cx, &preflight.at, pinned).await;
            self.clock_guard.check(self.clock.as_ref())?;
            let committer = TimelockCommitter::new(if preflight.null_consensus {
                CommitSizeLimit::Null {
                    mechanisms: u16::from(mechanisms),
                }
            } else {
                CommitSizeLimit::Yuma
            })?;
            commit = Some(
                committer
                    .commit_at(TimelockCommitRequest {
                        now: self.clock.now_unix(),
                        hotkey,
                        row: row.clone(),
                        version_key,
                        state: preflight.schedule.clone(),
                        reveal_period_epochs: preflight.reveal_period,
                        block_time,
                    })
                    .await?,
            );
        }
        let plan = submit::WeightCallPlan::new(
            cx.netuid,
            mech,
            row.clone(),
            version_key,
            commit.as_ref().map(|commit| (commit, cr_tag)),
        );
        // Declared encoder calls need the guard this attempt's binding runs.
        let guarded = self.semantic_guard_runs(mech)?;
        // A call no composer can produce pauses before the slot is reserved.
        let Some(composed) =
            self.compose_call(&cx.metrics, mech, &preflight, &composer, &plan, guarded)
        else {
            return Ok(());
        };
        let binding = self.prepare_binding(
            mech,
            row.clone(),
            commit.clone(),
            &preflight,
            composer.contract(),
        )?;
        let signed_before = slot::journal_attempts(binding.as_ref())?;
        let reservation = self.reserve_slot(
            mech,
            preflight.schedule.subnet_epoch_index,
            preflight.at.number,
        );
        if let Err(error) = self.persist(None) {
            // Nothing was dispatched, so nothing was signed.
            self.release_slot(mech, reservation);
            return Err(error);
        }
        let signing = submit::Signing::default();
        let result = submit::execute(
            cx,
            plan,
            composed,
            block_time,
            submit::SubmissionOptions {
                journal: binding.clone(),
                shield_period: self.shield_period,
            },
            &signing,
        )
        .await;
        let report = match result {
            Ok(report) => report,
            Err(error)
                if commit.is_some()
                    && (submit::invalid_reveal_round(&error)
                        || submit::incorrect_cr_version(&error)) =>
            {
                if binding.is_some() {
                    // Even a rejection cannot prove the same signed bytes were never
                    // accepted elsewhere. Durable recovery must exclude them first.
                    self.slots.entry(mech).or_default().uncertain = true;
                    return Err(error);
                }
                let mut error = error;
                let mut cr_retries = 0;
                let mut round_retried = false;
                let mut block_time = block_time;
                loop {
                    let recomputed = if submit::incorrect_cr_version(&error) {
                        // Root changed the tag between preflight and inclusion,
                        // or the best head has not caught up with it yet. The
                        // rejection is fee-free; wait, then re-select the tag
                        // from fresh state.
                        if cr_retries == CR_VERSION_RETRIES {
                            return Err(error);
                        }
                        let delay = block_backoff(block_time, cr_retries);
                        cr_retries += 1;
                        tracing::warn!(
                            mechanism = mech.0,
                            attempt = cr_retries,
                            ?delay,
                            %error,
                            "the chain rejected the commit-reveal version tag; re-reading it"
                        );
                        tokio::select! {
                            biased;
                            () = cx.shutdown.cancelled() => return Err(error),
                            () = tokio::time::sleep(delay) => {}
                        }
                        "rtao_weights_cr_version_recomputed_total"
                    } else {
                        if round_retried {
                            return Err(error);
                        }
                        round_retried = true;
                        "rtao_weights_reveal_round_recomputed_total"
                    };
                    let fresh = cx.client.at(BlockRef::Best).await?;
                    let fresh_pinned = crate::clock::Pinned::now();
                    let state = WeightPreflight::fetch(&fresh, cx.netuid, mech, hotkey).await?;
                    let uid = state.check_except_cr(version_key)?;
                    // Fresh state decides the tag, or pauses on unproven code.
                    let Some(cr_tag) = self.follow_cr_tag(&cx.metrics, mech, &state) else {
                        return Ok(());
                    };
                    let (count, current_owner) = reads::fetch(
                        &fresh,
                        (
                            st::MECHANISM_COUNT_CURRENT.at((cx.netuid,)),
                            st::SUBNET_OWNER_HOTKEY.at((cx.netuid,)),
                        ),
                    )
                    .await?;
                    anyhow::ensure!(
                        mech.0 < count.0 && count.0 <= 16,
                        "mechanism no longer exists after the rejected commitment"
                    );
                    let graph = cx.metagraph.load();
                    let row = prepare_row(input.clone(), &state, uid, cx.netuid, &graph)?;
                    let current_owner = *current_owner.raw();
                    if row_held(owner_rows, &row, uid, hotkey, current_owner, &graph) {
                        if let Some(attempt) = self.attempts.get_mut(&mech) {
                            attempt.outcome = WeightAttemptOutcome::HoldPrevious;
                        }
                        cx.metrics.increment("rtao_weights_hold_total");
                        return Ok(());
                    }
                    if row_held(
                        OwnerRowPolicy::Hold,
                        &row,
                        uid,
                        hotkey,
                        current_owner,
                        &graph,
                    ) {
                        cx.metrics.increment("rtao_weights_owner_row_total");
                        tracing::info!(
                            mechanism = mech.0,
                            "re-preparing an owner-only row under OwnerRowPolicy::Submit"
                        );
                    }
                    block_time = snapshot_slot_duration(&cx.client, &state.at).await?;
                    commit = if state.commit_reveal {
                        self.anchor_clock(cx, &state.at, fresh_pinned).await;
                        self.clock_guard.check(self.clock.as_ref())?;
                        Some(
                            TimelockCommitter::new(if state.null_consensus {
                                CommitSizeLimit::Null {
                                    mechanisms: u16::from(count.0),
                                }
                            } else {
                                CommitSizeLimit::Yuma
                            })?
                            .commit_at(TimelockCommitRequest {
                                now: self.clock.now_unix(),
                                hotkey,
                                row: row.clone(),
                                version_key,
                                state: state.schedule.clone(),
                                reveal_period_epochs: state.reveal_period,
                                block_time,
                            })
                            .await?,
                        )
                    } else {
                        None
                    };
                    // Fresh state selects and composes its own call.
                    let Some(composer) =
                        self.select_composer(&cx.metrics, mech, &state, encoder.as_ref())
                    else {
                        return Ok(());
                    };
                    let plan = submit::WeightCallPlan::new(
                        cx.netuid,
                        mech,
                        row.clone(),
                        version_key,
                        commit.as_ref().map(|commit| (commit, cr_tag)),
                    );
                    // This retry has no journal binding, so no semantic
                    // guard checks it: declared encoder calls refuse.
                    let Some(composed) =
                        self.compose_call(&cx.metrics, mech, &state, &composer, &plan, false)
                    else {
                        return Ok(());
                    };
                    let reservation =
                        self.reserve_slot(mech, state.schedule.subnet_epoch_index, state.at.number);
                    cx.metrics.increment(recomputed);
                    let signing = submit::Signing::default();
                    let retry = submit::execute(
                        cx,
                        plan,
                        composed,
                        block_time,
                        submit::SubmissionOptions {
                            journal: None,
                            shield_period: self.shield_period,
                        },
                        &signing,
                    )
                    .await;
                    let report = match retry {
                        Ok(report) => report,
                        Err(next)
                            if commit.is_some()
                                && (submit::invalid_reveal_round(&next)
                                    || submit::incorrect_cr_version(&next)) =>
                        {
                            error = next;
                            continue;
                        }
                        Err(error) => {
                            let unsigned = slot::signed_nothing(&signing, None, None);
                            if unsigned {
                                self.release_slot(mech, reservation);
                            }
                            self.slots.entry(mech).or_default().uncertain =
                                !unsigned && submit::uncertain(&error);
                            return Err(error);
                        }
                    };
                    if let Some(attempt) = self.attempts.get_mut(&mech) {
                        attempt.outcome = WeightAttemptOutcome::Submitted {
                            block_hash: report.block_hash,
                            block_number: report.block_number,
                        };
                    }
                    log_submitted(mech, report.block_number, commit.is_some());
                    if let Some(commit) = commit {
                        self.reveals
                            .record(
                                &cx.client,
                                cx.netuid,
                                mech,
                                hotkey,
                                row,
                                version_key,
                                state,
                                commit,
                                &report,
                                &cx.metrics,
                            )
                            .await?;
                    }
                    cx.metrics.increment("rtao_weights_submitted_total");
                    cx.metrics
                        .update_readiness(|ready| ready.last_weight_succeeded = true);
                    return Ok(());
                }
            }
            Err(error) => {
                // A failure that provably signed nothing returns the slot
                // (D-095). Durable work still reconciles through the journal
                // before its next dispatch.
                let unsigned = slot::signed_nothing(&signing, binding.as_ref(), signed_before);
                if unsigned {
                    self.release_slot(mech, reservation);
                }
                self.slots.entry(mech).or_default().uncertain =
                    binding.is_some() || (!unsigned && submit::uncertain(&error));
                self.persist(None)?;
                return Err(error);
            }
        };
        self.record_inclusion(&cx.client, mech, &report).await?;
        if let Some(attempt) = self.attempts.get_mut(&mech) {
            attempt.outcome = WeightAttemptOutcome::Submitted {
                block_hash: report.block_hash,
                block_number: report.block_number,
            };
        }
        log_submitted(mech, report.block_number, commit.is_some());
        if let Some(commit) = commit {
            self.reveals
                .record(
                    &cx.client,
                    cx.netuid,
                    mech,
                    hotkey,
                    row,
                    version_key,
                    preflight,
                    commit,
                    &report,
                    &cx.metrics,
                )
                .await?;
        }
        self.persist(binding.as_ref().map(|b| b.id()))?;
        cx.metrics.increment("rtao_weights_submitted_total");
        cx.metrics
            .update_readiness(|ready| ready.last_weight_succeeded = true);
        Ok(())
    }
}

impl WeightScheduler {
    /// Anchor the timelock clock, and feed the skew guard, from the chain
    /// before a timelocked commit (D-094).
    ///
    /// Reads `Timestamp.Now` at `at`, the best block the commit's preflight
    /// is pinned to, through the same verified snapshot read as every
    /// preflight value, and observes it as of `pinned`, the instant that
    /// block was pinned. An application that drives
    /// [`Self::on_finalized`] without `NeuronRuntime`'s clock worker
    /// therefore needs no anchor of its own. A failed read only warns: an
    /// anchored clock keeps its anchor, and an unanchored one refuses the
    /// commit before its cadence slot is reserved.
    async fn anchor_clock(
        &self,
        cx: &NeuronCx,
        at: &rtao_client::PinnedBlock,
        pinned: crate::clock::Pinned,
    ) {
        let snapshot = Snapshot::new(cx.client.clone(), at.clone());
        match reads::storage(&snapshot, &rtao_runtime::storage::timestamp::NOW, ()).await {
            Ok(timestamp) => {
                crate::clock::observe_chain_time(
                    self.clock.as_ref(),
                    &self.clock_guard,
                    timestamp,
                    pinned,
                );
            }
            Err(error) => {
                cx.metrics.increment("rtao_clock_sync_errors_total");
                tracing::warn!(%error, block = at.number, "chain clock anchor read failed; retaining the last anchor");
            }
        }
    }
}

/// Operators watch this line to confirm the validator is scoring on chain.
fn log_submitted(mech: MechId, block: u64, committed: bool) {
    if committed {
        tracing::info!(
            mechanism = mech.0,
            block,
            "weights committed; the chain reveals them after the reveal period"
        );
    } else {
        tracing::info!(mechanism = mech.0, block, "weights set");
    }
}

/// Deterministic default jitter, with direct rows governed by the chain limit.
pub fn default_cadence(hotkey: AccountId, tempo: u16, commit_reveal: bool) -> WeightCadence {
    if !commit_reveal {
        return WeightCadence::RateLimited { margin: 0 };
    }
    let digest = Blake2b::<U32>::digest(hotkey.0);
    let modulus = u64::from((tempo / 10).max(1));
    // Reduce all 256 digest bits rather than silently selecting one limb.
    let remainder = digest.iter().fold(0u64, |value, byte| {
        (value * 256 + u64::from(*byte)) % modulus
    });
    WeightCadence::OncePerEpoch {
        offset_blocks: 2 + remainder,
    }
}
fn due(cadence: WeightCadence, slot: &Slot, state: &WeightPreflight, uid: Uid) -> bool {
    cadence_opportunity(cadence, slot, state, uid) == WeightOpportunity::Ready
}
fn cadence_opportunity(
    cadence: WeightCadence,
    slot: &Slot,
    state: &WeightPreflight,
    uid: Uid,
) -> WeightOpportunity {
    if let Some(inclusion) = slot.last_attempt_block
        && state.at.number < inclusion
    {
        return WeightOpportunity::AwaitBlock {
            not_before_block: inclusion,
        };
    }
    if let Some((retry, _)) = slot.unsigned_retry
        && state.at.number < retry
    {
        return WeightOpportunity::AwaitBlock {
            not_before_block: retry,
        };
    }
    let last = state
        .last_update(uid)
        .max(slot.last_attempt_block.unwrap_or(0));
    if last != 0 && state.at.number.saturating_sub(last) < state.rate_limit {
        return WeightOpportunity::RateLimited {
            retry_after_block: last.saturating_add(state.rate_limit),
        };
    }
    match cadence {
        WeightCadence::OncePerEpoch { offset_blocks } => {
            if let Some(last_epoch) = slot.last_attempt_epoch
                && state.schedule.subnet_epoch_index <= last_epoch
            {
                WeightOpportunity::AwaitEpoch {
                    after_epoch: last_epoch,
                    offset_blocks,
                }
            } else if state
                .at
                .number
                .saturating_sub(state.schedule.last_epoch_block)
                < offset_blocks
            {
                WeightOpportunity::AwaitBlock {
                    not_before_block: state
                        .schedule
                        .last_epoch_block
                        .saturating_add(offset_blocks),
                }
            } else {
                WeightOpportunity::Ready
            }
        }
        WeightCadence::RateLimited { margin } => {
            let delay = state.rate_limit.saturating_add(margin).max(1);
            if last == 0 || state.at.number.saturating_sub(last) >= delay {
                WeightOpportunity::Ready
            } else {
                WeightOpportunity::RateLimited {
                    retry_after_block: last.saturating_add(delay),
                }
            }
        }
    }
}
fn prepare_row(
    input: WeightsInput,
    state: &WeightPreflight,
    uid: Uid,
    netuid: NetUid,
    graph: &rtao_metagraph::Metagraph,
) -> anyhow::Result<(Vec<Uid>, Vec<u16>)> {
    let (uids, weights, raw) = match input {
        WeightsInput::ByUid(row) => {
            let (uids, values) = row.into_iter().unzip();
            (uids, values, false)
        }
        WeightsInput::Parallel { uids, weights } => (uids, weights, false),
        WeightsInput::ByHotkey(row) => {
            let mut uids = Vec::with_capacity(row.len());
            let mut values = Vec::with_capacity(row.len());
            for (hotkey, weight) in row {
                uids.push(
                    graph
                        .by_hotkey(&hotkey)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "weight destination {hotkey} is absent from the last-good graph"
                            )
                        })?
                        .uid,
                );
                values.push(weight);
            }
            (uids, values, false)
        }
        WeightsInput::RawU16 { uids, values } => {
            anyhow::ensure!(
                state.null_consensus
                    && state.permits.iter().filter(|permit| **permit).count() == 1
                    && state.permits.get(usize::from(uid.0)) == Some(&true),
                "RawU16 requires Null consensus and this hotkey as the sole permit holder"
            );
            anyhow::ensure!(
                uids.len() == values.len(),
                "raw destinations and values differ in length"
            );
            (uids, values.into_iter().map(f64::from).collect(), true)
        }
    };
    anyhow::ensure!(
        uids.iter().collect::<BTreeSet<_>>().len() == uids.len(),
        "weight destinations contain duplicate UIDs"
    );
    anyhow::ensure!(
        weights
            .iter()
            .all(|weight| weight.is_finite() && *weight >= 0.0),
        "weights must be finite and non-negative"
    );
    anyhow::ensure!(
        uids.iter().all(|dest| dest.0 < state.population),
        "weight destination UID is outside the current subnet population"
    );
    if raw {
        // Pinned Null direct dispatch preserves zeros and checks length before
        // max/sum. The raw API promises those exact bytes, unlike Python's
        // positive-only conform path; never normalize or filter this vector.
        // The configured limit is a client guard: v478's chain getter currently
        // returns u16::MAX even when MaxWeightsLimit storage is smaller.
        let values = weights
            .into_iter()
            .map(|weight| weight as u16)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            values.iter().any(|value| *value > 0),
            "all raw weights are zero"
        );
        let self_weight = uids == [uid];
        let minimum = state.min_allowed_weights.min(state.population);
        anyhow::ensure!(
            (netuid.0 != 0 && self_weight) || values.len() >= usize::from(minimum),
            "raw row is below the chain minimum destination count"
        );
        let total: u128 = values.iter().map(|value| u128::from(*value)).sum();
        let maximum = u128::from(values.iter().copied().max().unwrap_or(0));
        anyhow::ensure!(
            self_weight
                || maximum * u128::from(u16::MAX) <= total * u128::from(state.max_weights_limit),
            "raw weights exceed the configured maximum share; change integer ratios or the subnet limit"
        );
        return Ok((uids, values));
    }
    Ok(conform(
        &uids,
        &weights,
        ConformState {
            uid,
            min_allowed_weights: state.min_allowed_weights.min(state.population),
            max_weight_limit: state.max_weights_limit,
        },
        netuid,
        false,
    )?)
}
/// The self/owner guard under `policy` (D-093) for a prepared row of the
/// validator `hotkey` at `uid`, with the subnet owner hotkey `owner`.
///
/// This validator's preflight UID is the owner's exactly when its hotkey is
/// the owner hotkey: the preflight is fresher than the last-good graph, so a
/// stale graph can neither hide an owner validator's own UID nor make a
/// non-owner validator's own UID look like the owner's. Any other destination
/// is the owner's when the last-good graph maps it to `owner`. Under
/// [`OwnerRowPolicy::Hold`] this is the guard earlier releases applied.
fn row_held(
    policy: OwnerRowPolicy,
    row: &(Vec<Uid>, Vec<u16>),
    uid: Uid,
    hotkey: AccountId,
    owner: AccountId,
    graph: &rtao_metagraph::Metagraph,
) -> bool {
    policy.holds(
        row.0.iter().copied().zip(row.1.iter().copied()),
        uid,
        |dest| {
            if dest == uid {
                hotkey == owner
            } else {
                graph.hotkey(dest) == Some(&owner)
            }
        },
    )
}
async fn snapshot_slot_duration(
    client: &Client,
    block: &rtao_client::PinnedBlock,
) -> anyhow::Result<f64> {
    let snapshot = Snapshot::new(client.clone(), block.clone());
    let millis = reads::slot_duration(&snapshot).await?;
    anyhow::ensure!(millis > 0, "Aura slot duration is zero");
    Ok(millis as f64 / 1000.0)
}
pub(super) fn number(value: &DynValue, name: &str) -> anyhow::Result<u64> {
    value
        .as_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("{name} is not a u64"))
}
fn boolean(value: &DynValue, name: &str) -> anyhow::Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("{name} is not a bool"))
}
fn integers(value: &DynValue, name: &str) -> anyhow::Result<Vec<u64>> {
    let Value::List(values) = value.as_core() else {
        anyhow::bail!("{name} is not a list")
    };
    values
        .iter()
        .map(|value| number(&DynValue::new(value.clone()), name))
        .collect()
}
fn booleans(value: &DynValue, name: &str) -> anyhow::Result<Vec<bool>> {
    let Value::List(values) = value.as_core() else {
        anyhow::bail!("{name} is not a list")
    };
    values
        .iter()
        .map(|value| boolean(&DynValue::new(value.clone()), name))
        .collect()
}
fn consensus_null(value: &DynValue) -> anyhow::Result<bool> {
    match value.as_core() {
        Value::Str(name) if name == "Null" => Ok(true),
        Value::Str(name) if name == "Yuma" => Ok(false),
        Value::Dict(fields) if fields.len() == 1 && fields[0].0 == Value::Str("Null".into()) => {
            Ok(true)
        }
        Value::Dict(fields) if fields.len() == 1 && fields[0].0 == Value::Str("Yuma".into()) => {
            Ok(false)
        }
        _ => anyhow::bail!("unknown SubnetEpochConsensus"),
    }
}

pub(crate) async fn run_scheduler<V: Validator>(
    validator: Arc<V>,
    state: StateCell<V::State>,
    cx: NeuronCx,
    guard: ClockGuard,
    clock: Arc<dyn Clock>,
    policy: WeightPolicy,
    journal: Option<Arc<rtao_client::journal::OperationJournal>>,
) -> anyhow::Result<()> {
    let journal = match journal {
        Some(journal) => journal,
        None => {
            let path = cx.state_dir.join("weights-journal.json");
            rtao_client::offload(move || rtao_client::journal::OperationJournal::open(path))
                .await??
        }
    };
    let mut scheduler = WeightScheduler::new(policy, clock, guard)?.with_journal(
        journal,
        cx.client.genesis(),
        cx.netuid,
        cx.hotkey.hotkey_address()?,
    )?;
    if let Some((contract, guard)) = validator.weight_semantics() {
        scheduler = scheduler.with_semantic_guard(contract, guard);
    }
    if let Some(encoder) = validator.weight_call_encoder() {
        scheduler = scheduler.with_call_encoder(encoder);
    }
    // The scheduler coalesces to the newest finalized head: the reveal
    // watcher and recovery follow history from their own cursors (D-096).
    let mut heads = cx
        .client
        .blocks_with(Finality::Finalized, rtao_client::Backfill::Latest)?;
    loop {
        let head = tokio::select! { biased; () = cx.shutdown.cancelled() => return Ok(()), next = heads.next() => next };
        let Some(head) = head else {
            anyhow::bail!("weight finalized-head stream ended")
        };
        let head = match head {
            Ok(head) => head,
            Err(error) => {
                tracing::warn!(%error, "weight head observer reconnecting");
                continue;
            }
        };
        let proposal_cx = cx.clone();
        if let Err(error) = scheduler
            .on_finalized_with(&cx, &head, |mech| {
                // One state view decides the row and its owner-row policy.
                let state = state.read();
                (
                    validator.weights(&proposal_cx, &state, mech),
                    validator.owner_row_policy(&proposal_cx, &state, mech),
                )
            })
            .await
        {
            cx.metrics.increment("rtao_weights_errors_total");
            cx.metrics
                .update_readiness(|ready| ready.last_weight_succeeded = false);
            tracing::warn!(%error, "weight scheduler will retry on the next finalized head");
        }
    }
}
