//! Complete immutable upstream vectors and pallet-derived safety-net cases.
#![allow(clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use rtao_core_ext::epoch_schedule::*;

// The vendored private core vectors are included verbatim rather than copied
// numeric expectations that could lose a case on a future pin bump.
mod timelock {
    pub mod epoch_schedule {
        pub use rtao_core_ext::epoch_schedule::{EpochScheduleError, EpochScheduleState};
    }
}
#[allow(dead_code)]
#[path = "../../../vendor/upstream/src/sdk/bittensor-core/src/timelock/epoch_schedule_vectors.rs"]
mod upstream_vectors;

fn state() -> EpochScheduleState {
    EpochScheduleState {
        last_epoch_block: 100,
        pending_epoch_at: 0,
        subnet_epoch_index: 5,
        tempo: 20,
        blocks_since_last_step: 0,
        current_block: 100,
    }
}
fn upstream(
    state: &EpochScheduleState,
) -> bittensor_core::timelock::epoch_schedule::EpochScheduleState {
    bittensor_core::timelock::epoch_schedule::EpochScheduleState {
        last_epoch_block: state.last_epoch_block,
        pending_epoch_at: state.pending_epoch_at,
        subnet_epoch_index: state.subnet_epoch_index,
        tempo: state.tempo,
        blocks_since_last_step: state.blocks_since_last_step,
        current_block: state.current_block,
    }
}

#[test]
fn every_upstream_prediction_and_commit_epoch_vector_matches() -> Result<(), EpochScheduleError> {
    for case in upstream_vectors::predict_vectors() {
        let result = predict_first_reveal_block(&case.state, case.reveal_period_epochs);
        match &case.expected_error {
            Some(error) => assert_eq!(
                result,
                Err(error.clone()),
                "{} ({})",
                case.name,
                case.source
            ),
            None => assert_eq!(
                result?,
                case.expected_reveal_block.unwrap_or_default(),
                "{} ({})",
                case.name,
                case.source
            ),
        }
    }
    for case in upstream_vectors::commit_epoch_vectors() {
        assert_eq!(
            current_epoch_pre_run_coinbase(&case.state, case.state.current_block + 1),
            case.expected_commit_epoch,
            "{}",
            case.name
        );
    }
    Ok(())
}

#[test]
fn scheduled_skip_fires_the_very_next_block_until_input_recovers() {
    let mut head = state();
    head.current_block = 119;
    head.blocks_since_last_step = 19;
    let skipped = EpochScheduleState {
        current_block: 120,
        last_epoch_block: 120,
        subnet_epoch_index: 6,
        blocks_since_last_step: 20,
        ..head
    };
    assert_eq!(skipped.blocks_since_last_step, 20);
    let again = EpochScheduleState {
        current_block: 121,
        last_epoch_block: 121,
        subnet_epoch_index: 7,
        blocks_since_last_step: 21,
        ..skipped
    };
    assert_eq!(again.subnet_epoch_index, 7);
    assert_eq!(again.blocks_since_last_step, 21);
    assert!(should_run_epoch(&again, 122));
    assert!(!bittensor_core::timelock::epoch_schedule::should_run_epoch(
        &upstream(&again),
        122
    ));
    let recovered = simulate_run_coinbase(&again, 122);
    assert_eq!(recovered.blocks_since_last_step, 0);
    assert_eq!(recovered.subnet_epoch_index, 8);
}

#[test]
fn owner_skip_uses_tempo_plus_one_minus_the_step_counter() {
    let mut head = state();
    head.current_block = 109;
    head.blocks_since_last_step = 9;
    head.pending_epoch_at = 110;
    let skipped = EpochScheduleState {
        current_block: 110,
        last_epoch_block: 110,
        subnet_epoch_index: 6,
        pending_epoch_at: 0,
        blocks_since_last_step: 10,
        ..head
    };
    assert_eq!(skipped.blocks_since_last_step, 10);
    let before = advance_blocks(&skipped, 111, 120);
    assert_eq!(before.subnet_epoch_index, 6);
    let fired = simulate_run_coinbase(&before, 121);
    assert_eq!(fired.subnet_epoch_index, 7);
    assert_eq!(
        121,
        110 + u64::from(skipped.tempo) + 1 - skipped.blocks_since_last_step
    );
}

#[test]
fn lowering_tempo_below_counter_fires_next_block() {
    let mut lowered = state();
    lowered.current_block = 130;
    lowered.last_epoch_block = 130;
    lowered.blocks_since_last_step = 30;
    lowered.tempo = 10;
    assert!(should_run_epoch(&lowered, 131));
    assert!(!bittensor_core::timelock::epoch_schedule::should_run_epoch(
        &upstream(&lowered),
        131
    ));
    let fired = simulate_run_coinbase(&lowered, 131);
    assert_eq!(fired.subnet_epoch_index, 6);
    assert_eq!(fired.last_epoch_block, 131);
}

#[test]
fn pending_deferral_is_preserved_and_age_cap_does_not_wrap() {
    let mut due = state();
    due.current_block = 119;
    due.blocks_since_last_step = u64::MAX;
    let deferred = EpochScheduleState {
        current_block: 120,
        pending_epoch_at: 121,
        blocks_since_last_step: 21,
        ..due.clone()
    };
    assert_eq!(deferred.pending_epoch_at, 121);
    assert_eq!(deferred.last_epoch_block, 100);
    assert_eq!(deferred.blocks_since_last_step, 21);
    assert_eq!(deferred.subnet_epoch_index, 5);
    assert!(should_run_epoch(&deferred, 121));
    assert!(bittensor_core::timelock::epoch_schedule::should_run_epoch(
        &upstream(&deferred),
        121
    ));
    let disabled = EpochScheduleState { tempo: 0, ..due };
    assert_eq!(
        simulate_run_coinbase(&disabled, 120).blocks_since_last_step,
        1
    );
    assert!(!should_run_epoch(&disabled, u64::MAX));
}

#[test]
fn inclusion_at_the_counter_boundary_uses_post_coinbase_epoch() -> Result<(), EpochScheduleError> {
    let mut head = state();
    head.last_epoch_block = 119;
    head.current_block = 119;
    head.blocks_since_last_step = 20;
    let inclusion = simulate_run_coinbase(&head, 120);
    assert_eq!(inclusion.subnet_epoch_index, 6);
    assert_eq!(predict_first_reveal_block(&head, 1)?, 140);
    assert_eq!(
        bittensor_core::timelock::epoch_schedule::predict_first_reveal_block(&upstream(&head), 1)
            .map_err(|_| EpochScheduleError::BoundExceeded)?,
        139
    );
    Ok(())
}

#[test]
fn ordinary_schedules_match_the_core_across_tempos_offsets_and_periods()
-> Result<(), EpochScheduleError> {
    for tempo in [1, 2, 5, 20, 100] {
        for offset in 0..u64::from(tempo) {
            for period in [0, 1, 2, 4] {
                let head = EpochScheduleState {
                    tempo,
                    current_block: 100 + offset,
                    blocks_since_last_step: offset,
                    ..state()
                };
                assert_eq!(
                    predict_first_reveal_block(&head, period),
                    bittensor_core::timelock::epoch_schedule::predict_first_reveal_block(
                        &upstream(&head),
                        period
                    )
                    .map_err(|_| EpochScheduleError::BoundExceeded)
                );
            }
        }
    }
    Ok(())
}

#[test]
fn overflowing_chain_positions_fail_closed() {
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
}
