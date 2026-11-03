//! A cadence slot reserved around one weight dispatch, and its release when
//! the dispatch provably signed nothing (D-095).
use super::{WeightScheduler, submit::Signing};
use rtao_client::journal::JournalBinding;
use rtao_primitives::MechId;

/// Longest wait, in blocks, before the next attempt after consecutive
/// attempts that signed nothing. The wait starts at one block and doubles.
pub(super) const UNSIGNED_RETRY_MAX_BLOCKS: u64 = 32;

/// The cadence state one attempt's reservation replaced.
#[derive(Clone, Copy, Debug)]
pub(super) struct Reservation {
    epoch: Option<u64>,
    block: Option<u64>,
    retry: Option<(u64, u32)>,
    at: u64,
}

/// Blocks to wait after the `released`th consecutive attempt that signed
/// nothing (from one).
fn unsigned_retry_delay(released: u32) -> u64 {
    (1u64 << released.saturating_sub(1).min(6)).min(UNSIGNED_RETRY_MAX_BLOCKS)
}

impl WeightScheduler {
    /// Reserve `mech`'s cadence slot for an attempt prepared at `block` in
    /// `epoch`, before anything is signed: an outcome the pool leaves unknown
    /// must never let the next finalized head submit again unconditionally.
    pub(super) fn reserve_slot(&mut self, mech: MechId, epoch: u64, block: u64) -> Reservation {
        let slot = self.slots.entry(mech).or_default();
        let reservation = Reservation {
            epoch: slot.last_attempt_epoch,
            block: slot.last_attempt_block,
            retry: slot.unsigned_retry,
            at: block,
        };
        slot.last_attempt_epoch = Some(epoch);
        slot.last_attempt_block = Some(block);
        slot.unsigned_retry = None;
        reservation
    }

    /// Give back a slot whose attempt signed nothing.
    ///
    /// The slot's epoch and block return to what they were before the
    /// reservation, so the same epoch can still be used. The next attempt
    /// waits one block after the first such failure, doubling up to
    /// [`UNSIGNED_RETRY_MAX_BLOCKS`] while they repeat, so a persistent
    /// refusal before signing cannot retry on every head.
    pub(super) fn release_slot(&mut self, mech: MechId, reservation: Reservation) {
        let slot = self.slots.entry(mech).or_default();
        slot.last_attempt_epoch = reservation.epoch;
        slot.last_attempt_block = reservation.block;
        let released = reservation
            .retry
            .map_or(1, |(_, released)| released.saturating_add(1));
        slot.unsigned_retry = Some((
            reservation
                .at
                .saturating_add(unsigned_retry_delay(released)),
            released,
        ));
    }
}

/// Signed attempts the journal holds for `binding`'s operation.
pub(super) fn journal_attempts(binding: Option<&JournalBinding>) -> anyhow::Result<Option<usize>> {
    binding
        .map(|binding| Ok(binding.record()?.attempts.len()))
        .transpose()
}

/// Whether a failed dispatch provably signed nothing.
///
/// Both must hold: the attempt's key source was never asked for a signature,
/// and a journal binding's operation holds exactly the `before` signed
/// attempts it held before dispatch (the journal records signed bytes before
/// they are sent). A journal that cannot be read counts as signed.
pub(super) fn signed_nothing(
    signing: &Signing,
    binding: Option<&JournalBinding>,
    before: Option<usize>,
) -> bool {
    if signing.signed() {
        return false;
    }
    match (binding, before) {
        (None, None) => true,
        (Some(binding), Some(before)) => binding
            .record()
            .is_ok_and(|record| record.attempts.len() == before),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Slot;
    use super::*;
    use crate::{ChainClock, ClockGuard, WeightPolicy};
    use std::sync::Arc;

    #[test]
    fn unsigned_retry_waits_one_block_then_doubles_to_the_bound() {
        let delays = (1..=9).map(unsigned_retry_delay).collect::<Vec<_>>();
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 32, 32, 32]);
        assert_eq!(unsigned_retry_delay(u32::MAX), UNSIGNED_RETRY_MAX_BLOCKS);
    }

    #[test]
    fn release_restores_the_previous_slot_and_a_signed_attempt_ends_the_backoff()
    -> anyhow::Result<()> {
        let mut scheduler = WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )?;
        let mech = MechId(0);
        // The first attempt of epoch 5 signed, so its reservation stands.
        let _signed = scheduler.reserve_slot(mech, 5, 100);
        let second = scheduler.reserve_slot(mech, 6, 110);
        scheduler.release_slot(mech, second);
        let slot = &scheduler.slots[&mech];
        assert_eq!(slot.last_attempt_epoch, Some(5));
        assert_eq!(slot.last_attempt_block, Some(100));
        assert_eq!(slot.unsigned_retry, Some((111, 1)));
        let third = scheduler.reserve_slot(mech, 6, 111);
        assert_eq!(scheduler.slots[&mech].unsigned_retry, None);
        scheduler.release_slot(mech, third);
        assert_eq!(scheduler.slots[&mech].unsigned_retry, Some((113, 2)));
        assert_eq!(scheduler.slots[&mech].last_attempt_epoch, Some(5));
        // An attempt that is not released clears the backoff.
        let _standing = scheduler.reserve_slot(mech, 6, 113);
        let slot = &scheduler.slots[&mech];
        assert_eq!(slot.last_attempt_epoch, Some(6));
        assert_eq!(slot.unsigned_retry, None);
        Ok(())
    }

    #[test]
    fn checkpoints_without_a_backoff_keep_their_shape() -> anyhow::Result<()> {
        let old: Slot = serde_json::from_value(serde_json::json!({
            "last_attempt_epoch": 5,
            "last_attempt_block": 100,
            "uncertain": true
        }))?;
        assert_eq!(old.unsigned_retry, None);
        assert_eq!(
            serde_json::to_value(&old)?,
            serde_json::json!({
                "last_attempt_epoch": 5,
                "last_attempt_block": 100,
                "uncertain": true
            })
        );
        let released = Slot {
            unsigned_retry: Some((111, 2)),
            ..Default::default()
        };
        let value = serde_json::to_value(&released)?;
        assert_eq!(value["unsigned_retry"], serde_json::json!([111, 2]));
        let back: Slot = serde_json::from_value(value)?;
        assert_eq!(back.unsigned_retry, Some((111, 2)));
        Ok(())
    }
}
