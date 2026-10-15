use bittensor_core::timelock::epoch_schedule::{
    EpochScheduleState, predict_first_reveal_block, should_run_epoch,
};

fn skipped() -> EpochScheduleState {
    EpochScheduleState {
        last_epoch_block: 120,
        pending_epoch_at: 0,
        subnet_epoch_index: 0,
        tempo: 20,
        blocks_since_last_step: 21,
        current_block: 120,
    }
}

#[test]
fn scheduled_skip_and_lowered_tempo_safety_net() {
    assert!(should_run_epoch(&skipped(), 121));
    let lowered = EpochScheduleState {
        last_epoch_block: 100,
        current_block: 100,
        blocks_since_last_step: 30,
        ..skipped()
    };
    assert!(should_run_epoch(&lowered, 101));
}

#[test]
fn inclusion_counter_fire_assigns_post_coinbase_epoch() {
    let head = EpochScheduleState {
        last_epoch_block: 90,
        current_block: 105,
        blocks_since_last_step: 20,
        ..skipped()
    };
    assert_eq!(predict_first_reveal_block(&head, 1), Ok(126));
}
