//! Commit-reveal version tag following (D-086, D-110, D-122).
//!
//! Every timelocked weight commit repeats the chain's
//! `CommitRevealWeightsVersion` storage value as a tag; a commit with any
//! other tag fails dispatch with `IncorrectCommitRevealVersion`. Runtime code
//! alone decides which payload a reveal decodes, so a changed stored tag does
//! not by itself say that the payload changed. This module decides which tag
//! a validator puts on its next commit from the stored tag, the live `:code`
//! storage hash and what the validator's own reveals showed, and it lists the
//! runtime code reviewed to reveal the [`NATIVE_PAYLOAD_VERSION`] payload.
//!
//! The module has no I/O and allocates nothing, so it builds for every target
//! the crate supports, `wasm32-unknown-unknown` included. A caller reads the
//! two storage values itself: `SubtensorModule.CommitRevealWeightsVersion`,
//! and, only when that differs from [`NATIVE_PAYLOAD_VERSION`], the `:code`
//! storage hash (`state_getStorageHash` of `0x3a636f6465`) at the same block.
//! [`TagEvidence`] keeps bounded reveal evidence in a fixed array for callers
//! without a checkpointed store of their own.
//!
//! Under [`Unreviewed::Follow`], the default, the stored tag is always
//! followed (D-122): the chain checks only that a commit's tag equals the
//! stored value, and runtime code decides the payload. The review list and
//! reveal evidence are observations reported in the decision's
//! [`TagBasis`], never preconditions. An unread `:code` hash follows as
//! [`TagBasis::CodeUnknown`]; an unexplained missed reveal follows as
//! [`TagBasis::Refuted`], which asks the caller to commit at most once per
//! subnet epoch ([`TagDecision::once_per_epoch`]) and to report the miss, so
//! the next reveal can prove the tag again. Only the explicit
//! [`Unreviewed::Pause`] opt-in pauses.

/// Timelocked payload format this build encodes.
///
/// Version 4 is the `WeightsTlockPayload` SCALE struct (hotkey, UIDs, values,
/// version key) encrypted to a drand quicknet round.
pub const NATIVE_PAYLOAD_VERSION: u16 = 4;

/// Runtime code reviewed to reveal the [`NATIVE_PAYLOAD_VERSION`] payload,
/// as `(spec_version, "0x" + hex blake2_256 of :code)`.
///
/// Each entry's pinned `coinbase/reveal_commits.rs` decodes
/// `WeightsTlockPayload` and never reads `CommitRevealWeightsVersion`, so
/// under exactly this code a commit may carry whatever tag the chain stores.
/// A runtime adoption adds the newly pinned code here once its reveal path is
/// checked. The list is an observation ([`TagBasis::Reviewed`]): under
/// [`Unreviewed::Follow`] [`select_tag`] follows the stored tag under any
/// code, listed or not.
pub const REVIEWED_RUNTIME_CODE: [(u32, &str); 2] = [
    (
        484,
        "0x23041f50d3529b359e9be3b90f3ca40474678e834e97f49fb401d0c691097760",
    ),
    (
        486,
        "0x49781ac4cfda0ac6cf6e53cfceea7c728394d9c28da7881f47e3fbfbc239800f",
    ),
];

/// One reviewed runtime: its spec version and `:code` storage hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReviewedRuntime {
    /// Runtime spec version.
    pub spec_version: u32,
    /// BLAKE2b-256 of the runtime's `:code`.
    pub code_hash: [u8; 32],
}

/// [`REVIEWED_RUNTIME_CODE`] with the hashes decoded at compile time.
pub const REVIEWED_RUNTIMES: [ReviewedRuntime; REVIEWED_RUNTIME_CODE.len()] = {
    let mut table = [ReviewedRuntime {
        spec_version: 0,
        code_hash: [0; 32],
    }; REVIEWED_RUNTIME_CODE.len()];
    let mut index = 0;
    while index < table.len() {
        let (spec_version, hash) = REVIEWED_RUNTIME_CODE[index];
        table[index] = ReviewedRuntime {
            spec_version,
            code_hash: hash32(hash),
        };
        index += 1;
    }
    table
};

/// Decode a `0x`-prefixed 64-digit hex string at compile time.
// Only evaluated in the constant above: a malformed entry fails the build.
#[allow(clippy::panic)]
const fn hash32(text: &str) -> [u8; 32] {
    const fn digit(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => panic!("reviewed runtime hash is not hex"),
        }
    }
    let bytes = text.as_bytes();
    assert!(
        bytes.len() == 66 && bytes[0] == b'0' && bytes[1] == b'x',
        "reviewed runtime hash is not 0x and 64 hex digits"
    );
    let mut out = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        out[index] = digit(bytes[2 + 2 * index]) << 4 | digit(bytes[3 + 2 * index]);
        index += 1;
    }
    out
}

/// Spec version of the reviewed runtime whose `:code` hash is `code_hash`.
pub fn reviewed_runtime(code_hash: &[u8; 32]) -> Option<u32> {
    REVIEWED_RUNTIMES
        .iter()
        .find(|runtime| runtime.code_hash == *code_hash)
        .map(|runtime| runtime.spec_version)
}

/// What this validator's own reveals showed for one runtime code under one
/// stored tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RevealEvidence {
    /// No commitment carrying the tag has settled under the code.
    #[default]
    None,
    /// A commitment carrying the tag revealed under the code (a success
    /// event bound to the exact ciphertext in the parent queue).
    Revealed,
    /// The latest commitment carrying the tag under the code missed its
    /// reveal with no drand, epoch, period or chain-check explanation.
    Refuted,
}

/// What [`select_tag`] does with a changed tag under code that is neither
/// reviewed nor proven by a reveal, or whose reveal missed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unreviewed {
    /// Follow the stored tag under any code and report why
    /// ([`TagBasis`]). A refuting miss is reported and limits commits to one
    /// per subnet epoch; it never stops them (D-122).
    #[default]
    Follow,
    /// Opt-in: pause commits until the code is reviewed or a reveal proves
    /// it, and after a refuting miss or without a `:code` hash, the behavior
    /// of D-086 before D-110. This locks timelocked commits to
    /// [`REVIEWED_RUNTIME_CODE`] and to outstanding evidence: a validator
    /// with nothing outstanding cannot gather a reveal, so it waits for a
    /// release that lists the code or for the chain to return to
    /// [`NATIVE_PAYLOAD_VERSION`].
    Pause,
}

/// Why [`select_tag`] follows a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TagBasis {
    /// The stored tag is [`NATIVE_PAYLOAD_VERSION`], or commit-reveal is off.
    Native,
    /// The live code is listed in [`REVIEWED_RUNTIME_CODE`].
    Reviewed {
        /// Spec version of the reviewed entry.
        spec_version: u32,
    },
    /// One of this validator's commitments revealed under this code and tag.
    Revealed,
    /// Neither reviewed nor proven yet. Operators should know: the next
    /// reveal under this code either proves the tag or refutes it.
    Unreviewed,
    /// The latest commitment carrying this tag under this code missed its
    /// reveal with no other explanation. The tag is still the only one the
    /// chain accepts, so it is followed; the caller commits at most once per
    /// subnet epoch and reports the miss, and a later reveal proves the tag
    /// again (D-122).
    Refuted,
    /// The caller did not read the live `:code` hash, so neither the review
    /// list nor reveal evidence applies; the stored tag is followed (D-122).
    CodeUnknown,
}

/// Why [`select_tag`] pauses timelocked commits. Only the
/// [`Unreviewed::Pause`] opt-in pauses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TagPause {
    /// The tag is not native and the caller did not read the live `:code`
    /// hash, so neither the review list nor reveal evidence can apply.
    CodeUnknown,
    /// A commitment carrying this tag under this code missed its reveal with
    /// no other explanation.
    Refuted,
    /// [`Unreviewed::Pause`] and the code is neither reviewed nor proven.
    Unreviewed,
}

/// The tag for the next timelocked commit, or why commits pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagDecision {
    /// Put `tag` on the commit.
    Follow {
        /// Tag to put on the commit.
        tag: u16,
        /// Why it is followed.
        basis: TagBasis,
    },
    /// Do not commit now; other duties continue and the next head decides
    /// again.
    Pause {
        /// The stored tag that is not followed.
        tag: u16,
        /// Why.
        reason: TagPause,
    },
}
impl TagDecision {
    /// The tag to commit with, if any.
    pub const fn tag(self) -> Option<u16> {
        match self {
            Self::Follow { tag, .. } => Some(tag),
            Self::Pause { .. } => None,
        }
    }
    /// Why the tag is followed, if it is.
    pub const fn basis(self) -> Option<TagBasis> {
        match self {
            Self::Follow { basis, .. } => Some(basis),
            Self::Pause { .. } => None,
        }
    }
    /// Whether the caller should make at most one timelocked commit per
    /// subnet epoch under this decision: the tag is followed although the
    /// latest commitment carrying it under this code missed its reveal
    /// ([`TagBasis::Refuted`]). One commit per epoch is the evidence a
    /// validator needs to see the tag reveal again (D-122).
    pub const fn once_per_epoch(self) -> bool {
        matches!(
            self,
            Self::Follow {
                basis: TagBasis::Refuted,
                ..
            }
        )
    }
}

/// Decide the tag for a timelocked commit (D-110, D-122).
///
/// - `commit_reveal`: `CommitRevealWeightsEnabled` for the subnet; when off,
///   no commit is made and the native tag is returned for completeness.
/// - `stored_tag`: `CommitRevealWeightsVersion` at the block the commit is
///   prepared against.
/// - `code_hash`: the `:code` storage hash at that block; needed only when
///   `stored_tag` is not [`NATIVE_PAYLOAD_VERSION`].
/// - `evidence`: what this validator's reveals showed for that code and tag.
///
/// Under [`Unreviewed::Follow`] the stored tag is always followed and the
/// basis reports what is known: refutation wins over review (reviewed code
/// whose reveals are seen failing reports [`TagBasis::Refuted`] until a later
/// reveal proves it again), then review, then a reveal, then nothing. Under
/// [`Unreviewed::Pause`] a refuted, unreviewed and unproven, or unidentified
/// code pauses instead.
pub fn select_tag(
    commit_reveal: bool,
    stored_tag: u16,
    code_hash: Option<&[u8; 32]>,
    evidence: RevealEvidence,
    unreviewed: Unreviewed,
) -> TagDecision {
    if !commit_reveal || stored_tag == NATIVE_PAYLOAD_VERSION {
        return TagDecision::Follow {
            tag: NATIVE_PAYLOAD_VERSION,
            basis: TagBasis::Native,
        };
    }
    let pause = |reason| TagDecision::Pause {
        tag: stored_tag,
        reason,
    };
    let follow = |basis| TagDecision::Follow {
        tag: stored_tag,
        basis,
    };
    let Some(code) = code_hash else {
        return match unreviewed {
            Unreviewed::Follow => follow(TagBasis::CodeUnknown),
            Unreviewed::Pause => pause(TagPause::CodeUnknown),
        };
    };
    match (evidence, reviewed_runtime(code), unreviewed) {
        (RevealEvidence::Refuted, _, Unreviewed::Follow) => follow(TagBasis::Refuted),
        (RevealEvidence::Refuted, _, Unreviewed::Pause) => pause(TagPause::Refuted),
        (_, Some(spec_version), _) => follow(TagBasis::Reviewed { spec_version }),
        (RevealEvidence::Revealed, None, _) => follow(TagBasis::Revealed),
        (RevealEvidence::None, None, Unreviewed::Follow) => follow(TagBasis::Unreviewed),
        (RevealEvidence::None, None, Unreviewed::Pause) => pause(TagPause::Unreviewed),
    }
}

/// One retained reveal observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EvidenceEntry {
    /// `:code` storage hash at the reveal block's parent.
    pub code_hash: [u8; 32],
    /// `CommitRevealWeightsVersion` stored at that parent.
    pub tag: u16,
    /// The commitment missed its reveal with no other explanation.
    pub missed: bool,
}

/// Bounded reveal evidence in a fixed array of `N` entries, without
/// allocation.
///
/// The latest observation for a code and tag replaces the earlier one. When
/// full, the oldest positive entry is dropped first: refutations outlive
/// positive evidence, since dropping one would resume a tag whose reveals
/// were seen failing. Native evidence (the [`NATIVE_PAYLOAD_VERSION`] tag)
/// needs no entry and is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagEvidence<const N: usize> {
    entries: [Option<EvidenceEntry>; N],
    len: usize,
}
impl<const N: usize> Default for TagEvidence<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> TagEvidence<N> {
    /// An empty store.
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            len: 0,
        }
    }
    /// The evidence for `code_hash` under `tag`.
    pub fn lookup(&self, code_hash: &[u8; 32], tag: u16) -> RevealEvidence {
        match self
            .iter()
            .find(|entry| entry.code_hash == *code_hash && entry.tag == tag)
        {
            Some(entry) if entry.missed => RevealEvidence::Refuted,
            Some(_) => RevealEvidence::Revealed,
            None => RevealEvidence::None,
        }
    }
    /// Record that a commitment tagged `tag` under `code_hash` revealed
    /// (`missed` false) or missed with no other explanation (`missed` true).
    pub fn record(&mut self, code_hash: [u8; 32], tag: u16, missed: bool) {
        if tag == NATIVE_PAYLOAD_VERSION || N == 0 {
            return;
        }
        let existing = self
            .iter()
            .position(|entry| entry.code_hash == code_hash && entry.tag == tag);
        if let Some(position) = existing {
            self.remove(position);
        }
        if self.len == N {
            let oldest = self.iter().position(|entry| !entry.missed);
            self.remove(oldest.unwrap_or(0));
        }
        self.entries[self.len] = Some(EvidenceEntry {
            code_hash,
            tag,
            missed,
        });
        self.len += 1;
    }
    /// Retained entries, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = EvidenceEntry> + '_ {
        self.entries[..self.len].iter().flatten().copied()
    }
    /// Number of retained entries.
    pub const fn len(&self) -> usize {
        self.len
    }
    /// Whether nothing is retained.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn remove(&mut self, position: usize) {
        self.entries[position..self.len].rotate_left(1);
        self.len -= 1;
        self.entries[self.len] = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNKNOWN: [u8; 32] = [7; 32];

    #[test]
    fn reviewed_table_decodes_every_hash() {
        for (runtime, (spec, text)) in REVIEWED_RUNTIMES.iter().zip(REVIEWED_RUNTIME_CODE) {
            assert_eq!(runtime.spec_version, spec);
            let hex: String = runtime
                .code_hash
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(format!("0x{hex}"), text);
            assert_eq!(reviewed_runtime(&runtime.code_hash), Some(spec));
        }
        assert_eq!(reviewed_runtime(&UNKNOWN), None);
    }

    #[test]
    fn native_tag_and_disabled_commit_reveal_need_no_code() {
        for (enabled, tag) in [(true, 4), (false, 5), (false, 4)] {
            assert_eq!(
                select_tag(enabled, tag, None, RevealEvidence::None, Unreviewed::Pause),
                TagDecision::Follow {
                    tag: 4,
                    basis: TagBasis::Native
                }
            );
        }
    }

    #[test]
    fn the_stored_tag_is_followed_under_any_code_by_default() {
        let reviewed = REVIEWED_RUNTIMES[0];
        for (code, evidence, basis) in [
            (Some(&UNKNOWN), RevealEvidence::None, TagBasis::Unreviewed),
            (Some(&UNKNOWN), RevealEvidence::Revealed, TagBasis::Revealed),
            (Some(&UNKNOWN), RevealEvidence::Refuted, TagBasis::Refuted),
            (
                Some(&reviewed.code_hash),
                RevealEvidence::None,
                TagBasis::Reviewed {
                    spec_version: reviewed.spec_version,
                },
            ),
            (
                Some(&reviewed.code_hash),
                RevealEvidence::Refuted,
                TagBasis::Refuted,
            ),
            (None, RevealEvidence::None, TagBasis::CodeUnknown),
            (None, RevealEvidence::Refuted, TagBasis::CodeUnknown),
        ] {
            let decision = select_tag(true, 9, code, evidence, Unreviewed::default());
            assert_eq!(decision, TagDecision::Follow { tag: 9, basis });
            assert_eq!(decision.tag(), Some(9));
            assert_eq!(decision.basis(), Some(basis));
            assert_eq!(decision.once_per_epoch(), basis == TagBasis::Refuted);
        }
    }

    #[test]
    fn the_pause_opt_in_pauses_unproven_refuted_and_unidentified_code() {
        let follow = select_tag(
            true,
            5,
            Some(&UNKNOWN),
            RevealEvidence::None,
            Unreviewed::Follow,
        );
        assert_eq!(
            follow,
            TagDecision::Follow {
                tag: 5,
                basis: TagBasis::Unreviewed
            }
        );
        assert_eq!(follow.tag(), Some(5));
        assert_eq!(
            select_tag(
                true,
                5,
                Some(&UNKNOWN),
                RevealEvidence::Revealed,
                Unreviewed::Pause
            ),
            TagDecision::Follow {
                tag: 5,
                basis: TagBasis::Revealed
            }
        );
        assert_eq!(
            select_tag(
                true,
                5,
                Some(&UNKNOWN),
                RevealEvidence::Refuted,
                Unreviewed::Pause
            ),
            TagDecision::Pause {
                tag: 5,
                reason: TagPause::Refuted
            }
        );
        assert_eq!(
            select_tag(true, 5, None, RevealEvidence::None, Unreviewed::Pause),
            TagDecision::Pause {
                tag: 5,
                reason: TagPause::CodeUnknown
            }
        );
        assert_eq!(
            select_tag(
                true,
                5,
                Some(&UNKNOWN),
                RevealEvidence::None,
                Unreviewed::Pause
            ),
            TagDecision::Pause {
                tag: 5,
                reason: TagPause::Unreviewed
            }
        );
    }

    #[test]
    fn reviewed_code_is_followed_and_a_refutation_is_reported() {
        let reviewed = REVIEWED_RUNTIMES[REVIEWED_RUNTIMES.len() - 1];
        assert_eq!(
            select_tag(
                true,
                6,
                Some(&reviewed.code_hash),
                RevealEvidence::None,
                Unreviewed::Pause
            ),
            TagDecision::Follow {
                tag: 6,
                basis: TagBasis::Reviewed {
                    spec_version: reviewed.spec_version
                }
            }
        );
        let refuted = select_tag(
            true,
            6,
            Some(&reviewed.code_hash),
            RevealEvidence::Refuted,
            Unreviewed::Follow,
        );
        assert_eq!(refuted.tag(), Some(6));
        assert!(refuted.once_per_epoch());
        assert_eq!(
            select_tag(
                true,
                6,
                Some(&reviewed.code_hash),
                RevealEvidence::Refuted,
                Unreviewed::Pause
            )
            .tag(),
            None
        );
    }

    #[test]
    fn evidence_is_bounded_keeps_refutations_and_replaces_by_code_and_tag() {
        let code = |n: u8| [n; 32];
        let mut evidence = TagEvidence::<3>::new();
        evidence.record(code(1), 4, true);
        assert!(evidence.is_empty(), "native evidence is never retained");
        evidence.record(code(1), 5, true);
        evidence.record(code(2), 5, false);
        evidence.record(code(3), 5, false);
        assert_eq!(evidence.lookup(&code(1), 5), RevealEvidence::Refuted);
        assert_eq!(evidence.lookup(&code(1), 6), RevealEvidence::None);
        // Full: the oldest positive entry goes, the refutation stays.
        evidence.record(code(4), 5, false);
        assert_eq!(evidence.len(), 3);
        assert_eq!(evidence.lookup(&code(2), 5), RevealEvidence::None);
        assert_eq!(evidence.lookup(&code(1), 5), RevealEvidence::Refuted);
        // A later reveal replaces the refutation in place of a new entry.
        evidence.record(code(1), 5, false);
        assert_eq!(evidence.len(), 3);
        assert_eq!(evidence.lookup(&code(1), 5), RevealEvidence::Revealed);
        assert_eq!(
            evidence
                .iter()
                .map(|entry| entry.code_hash[0])
                .collect::<Vec<_>>(),
            [3, 4, 1]
        );
        // All refutations: the oldest goes.
        let mut refuted = TagEvidence::<2>::new();
        for n in 1..=3 {
            refuted.record(code(n), 5, true);
        }
        assert_eq!(
            refuted
                .iter()
                .map(|entry| entry.code_hash[0])
                .collect::<Vec<_>>(),
            [2, 3]
        );
        let mut none = TagEvidence::<0>::new();
        none.record(code(1), 5, true);
        assert!(none.is_empty());
    }
}
