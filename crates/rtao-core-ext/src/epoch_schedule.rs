// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/timelock/epoch_schedule.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! UPSTREAM-TARGET: timelock::epoch_schedule (PR-U5)
//! UPSTREAM-PROBE: vectors epoch_schedule
//! UPSTREAM-FEATURES: none
//!
//! Pallet-faithful epoch scheduling for timelocked weight commits.
//!
//! Simulation assumes successful epochs and does not predict future input
//! inconsistency or `MaxEpochsPerBlock` deferrals. The reveal watcher must
//! follow the epoch actually assigned to the included commit.

use bittensor_core::timelock::constants::{COMMIT_INCLUSION_BLOCK_OFFSET, MAX_TEMPO_U64};
pub use bittensor_core::timelock::epoch_schedule::{EpochScheduleError, EpochScheduleState};

/// Whether the pallet's epoch predicate is true for the supplied stored state.
pub fn should_run_epoch(state: &EpochScheduleState, block: u64) -> bool {
    if state.tempo == 0 {
        return false;
    }
    if state.pending_epoch_at > 0 && block >= state.pending_epoch_at {
        return true;
    }
    if state.blocks_since_last_step > u64::from(state.tempo) {
        return true;
    }
    block.saturating_sub(state.last_epoch_block) >= u64::from(state.tempo)
}

/// The look-ahead epoch seen by reveal processing before `run_coinbase`.
pub fn current_epoch_pre_run_coinbase(state: &EpochScheduleState, block: u64) -> u64 {
    if should_run_epoch(state, block) {
        state.subnet_epoch_index.saturating_add(1)
    } else {
        state.subnet_epoch_index
    }
}

/// Advance one block, applying the pallet's bounded counter and a successful epoch.
pub fn simulate_run_coinbase(state: &EpochScheduleState, block: u64) -> EpochScheduleState {
    let mut next = state.clone();
    next.blocks_since_last_step = next
        .blocks_since_last_step
        .saturating_add(1)
        .min(u64::from(next.tempo).saturating_add(1));
    next.current_block = block;
    if should_run_epoch(&next, block) {
        next.last_epoch_block = block;
        next.pending_epoch_at = 0;
        next.subnet_epoch_index = next.subnet_epoch_index.saturating_add(1);
        next.blocks_since_last_step = 0;
    }
    next
}

/// Advance every block in `start..=end`; an empty range returns the original state.
pub fn advance_blocks(from: &EpochScheduleState, start: u64, end: u64) -> EpochScheduleState {
    let mut state = from.clone();
    for block in start..=end {
        state = simulate_run_coinbase(&state, block);
    }
    state
}

/// Predict the first block of the commit's actual reveal epoch.
///
/// Commit extrinsics run after `run_coinbase`, while automatic reveal processing
/// runs before it. Counter-threshold fires can occur only after the coinbase
/// increment, so compute the commit epoch from the simulated post-coinbase state.
pub fn predict_first_reveal_block(
    head_state: &EpochScheduleState,
    reveal_period_epochs: u64,
) -> Result<u64, EpochScheduleError> {
    if head_state.tempo == 0 {
        return Err(EpochScheduleError::TempoIsZero);
    }
    let first_block = head_state
        .current_block
        .checked_add(1)
        .ok_or(EpochScheduleError::BoundExceeded)?;
    let extrinsic_block = head_state
        .current_block
        .checked_add(COMMIT_INCLUSION_BLOCK_OFFSET)
        .ok_or(EpochScheduleError::BoundExceeded)?;
    let before_extrinsic =
        advance_blocks(head_state, first_block, extrinsic_block.saturating_sub(1));
    let after_extrinsic = simulate_run_coinbase(&before_extrinsic, extrinsic_block);
    let commit_epoch = current_epoch_pre_run_coinbase(&after_extrinsic, extrinsic_block);
    let target_epoch = commit_epoch
        .checked_add(reveal_period_epochs)
        .ok_or(EpochScheduleError::BoundExceeded)?;
    let budget = reveal_period_epochs
        .checked_mul(MAX_TEMPO_U64)
        .and_then(|value| value.checked_add(MAX_TEMPO_U64))
        .ok_or(EpochScheduleError::BoundExceeded)?;
    let end = extrinsic_block
        .checked_add(budget)
        .ok_or(EpochScheduleError::BoundExceeded)?;
    let mut previous = before_extrinsic;
    for block in extrinsic_block..=end {
        if current_epoch_pre_run_coinbase(&previous, block) == target_epoch {
            return Ok(block);
        }
        previous = simulate_run_coinbase(&previous, block);
    }
    Err(EpochScheduleError::BoundExceeded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> EpochScheduleState {
        EpochScheduleState {
            last_epoch_block: 100,
            pending_epoch_at: 0,
            subnet_epoch_index: 0,
            tempo: 20,
            blocks_since_last_step: 0,
            current_block: 100,
        }
    }

    #[test]
    fn overflowing_budget_and_search_end_fail_before_iteration() {
        assert_eq!(
            predict_first_reveal_block(&state(), u64::MAX),
            Err(EpochScheduleError::BoundExceeded)
        );
        let near_end = EpochScheduleState {
            current_block: u64::MAX - 2,
            last_epoch_block: u64::MAX - 2,
            ..state()
        };
        assert_eq!(
            predict_first_reveal_block(&near_end, 1),
            Err(EpochScheduleError::BoundExceeded)
        );
    }

    #[test]
    fn all_upstream_prediction_vectors() {
        // Complete v478 epoch_schedule_vectors.rs suite. Its module is private
        // and cfg(test) upstream, so pin the public inputs and expected outputs.
        let cases = [
            (
                "tempo_zero",
                EpochScheduleState {
                    last_epoch_block: 10,
                    current_block: 10,
                    tempo: 0,
                    ..state()
                },
                Err(EpochScheduleError::TempoIsZero),
            ),
            (
                "cycle_reset",
                EpochScheduleState {
                    last_epoch_block: 10,
                    current_block: 10,
                    tempo: 50,
                    ..state()
                },
                Ok(60),
            ),
            (
                "pending_fires_before_auto",
                EpochScheduleState {
                    last_epoch_block: 80,
                    pending_epoch_at: 95,
                    current_block: 91,
                    ..state()
                },
                Ok(95),
            ),
        ];
        for (name, state, expected) in cases {
            assert_eq!(predict_first_reveal_block(&state, 1), expected, "{name}");
        }
        let head = EpochScheduleState {
            current_block: 120,
            tempo: 50,
            ..state()
        };
        let after = simulate_run_coinbase(&head, 121);
        assert_eq!(current_epoch_pre_run_coinbase(&after, 121), 0);
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // Deliberate comparison with the upstream defect.
    fn skipped_scheduled_slot_fires_next_block() {
        let skipped = EpochScheduleState {
            last_epoch_block: 120,
            current_block: 120,
            blocks_since_last_step: 21,
            ..state()
        };
        assert!(should_run_epoch(&skipped, 121));
        assert!(!bittensor_core::timelock::epoch_schedule::should_run_epoch(
            &skipped, 121
        ));
        assert_eq!(simulate_run_coinbase(&skipped, 121).subnet_epoch_index, 1);
    }

    #[test]
    fn skipped_owner_slot_fires_at_counter_threshold() {
        let skipped = EpochScheduleState {
            last_epoch_block: 90,
            current_block: 90,
            blocks_since_last_step: 5,
            ..state()
        };
        assert_eq!(advance_blocks(&skipped, 91, 105).subnet_epoch_index, 0);
        assert_eq!(advance_blocks(&skipped, 91, 106).subnet_epoch_index, 1);
    }

    #[test]
    fn lowered_tempo_fires_next_block_and_counter_is_bounded() {
        let lowered = EpochScheduleState {
            blocks_since_last_step: 30,
            ..state()
        };
        assert!(should_run_epoch(&lowered, 101));
        let disabled = EpochScheduleState {
            tempo: 0,
            blocks_since_last_step: u64::MAX,
            ..state()
        };
        let next = simulate_run_coinbase(&disabled, 101);
        assert_eq!(next.blocks_since_last_step, 1);
        assert_eq!(next.subnet_epoch_index, 0);
    }

    #[test]
    fn inclusion_counter_fire_assigns_post_coinbase_epoch() {
        // An owner-triggered skip at 90 left a step counter of 5. At head 105
        // the counter is 20. Coinbase at inclusion 106 increments it to 21
        // and succeeds, before the commit extrinsic is applied.
        let head = EpochScheduleState {
            last_epoch_block: 90,
            current_block: 105,
            blocks_since_last_step: 20,
            ..state()
        };
        assert!(!should_run_epoch(&head, 106));
        let after = simulate_run_coinbase(&head, 106);
        assert_eq!(after.subnet_epoch_index, 1);
        assert_eq!(current_epoch_pre_run_coinbase(&after, 106), 1);
        assert_eq!(predict_first_reveal_block(&head, 1), Ok(126));
    }

    #[test]
    fn prediction_refuses_overflow_and_disabled_tempo() {
        assert_eq!(
            predict_first_reveal_block(
                &EpochScheduleState {
                    current_block: u64::MAX,
                    ..state()
                },
                1
            ),
            Err(EpochScheduleError::BoundExceeded)
        );
        assert_eq!(
            predict_first_reveal_block(
                &EpochScheduleState {
                    subnet_epoch_index: u64::MAX,
                    ..state()
                },
                1
            ),
            Err(EpochScheduleError::BoundExceeded)
        );
        assert_eq!(
            predict_first_reveal_block(
                &EpochScheduleState {
                    tempo: 0,
                    ..state()
                },
                1
            ),
            Err(EpochScheduleError::TempoIsZero)
        );
    }
}
