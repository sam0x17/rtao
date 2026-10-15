//! Round arithmetic and upstream payload framing with an injected clock.
use bittensor_core::timelock::{
    WeightsTlockPayload,
    constants::{DRAND_PERIOD, GENESIS_TIME},
};
use parity_scale_codec::Decode;
use rtao_core_ext::{
    epoch_schedule::EpochScheduleState,
    timelock_commit::{commit_payload, generate_commit_v2_at, reveal_round_at},
};

fn state() -> EpochScheduleState {
    EpochScheduleState {
        last_epoch_block: 100,
        pending_epoch_at: 0,
        subnet_epoch_index: 5,
        tempo: 20,
        blocks_since_last_step: 1,
        current_block: 101,
    }
}

#[test]
fn explicit_clock_preserves_the_core_round_operation_order() -> Result<(), bittensor_core::CoreError>
{
    let now = GENESIS_TIME as f64 + 1000.25;
    let expected = ((now + 22.0 * 12.0 - GENESIS_TIME as f64) / DRAND_PERIOD as f64).floor() as u64;
    assert_eq!(reveal_round_at(now, &state(), 1, 12.0)?, expected);
    assert_eq!(reveal_round_at(now, &state(), 1, 0.25)?, 335);
    assert_eq!(reveal_round_at(0.0, &state(), 1, 0.25)?, 1);
    Ok(())
}

#[test]
fn fractional_time_floor_boundaries_are_exact() -> Result<(), bittensor_core::CoreError> {
    let now = GENESIS_TIME as f64 + 100.5;
    assert_eq!(reveal_round_at(now, &state(), 1, 0.25)?, 35);
    assert_eq!(reveal_round_at(now + 2.0, &state(), 1, 0.25)?, 36);
    Ok(())
}

#[test]
fn invalid_times_and_overflows_fail_without_encrypting() {
    for now in [f64::NAN, f64::INFINITY, -1.0, f64::MAX] {
        assert!(reveal_round_at(now, &state(), 1, 12.0).is_err());
    }
    for slot in [f64::NAN, f64::INFINITY, -1.0, 0.0, f64::MAX] {
        assert!(reveal_round_at(1.0, &state(), 1, slot).is_err());
    }
    let near_max = EpochScheduleState {
        current_block: u64::MAX,
        ..state()
    };
    assert!(reveal_round_at(1.0, &near_max, 1, 12.0).is_err());
    let invalid = WeightsTlockPayload {
        hotkey: vec![7; 32],
        uids: vec![1],
        values: vec![],
        version_key: 1,
    };
    assert!(
        generate_commit_v2_at(
            1.0,
            invalid.uids,
            invalid.values,
            invalid.version_key,
            state(),
            1,
            12.0,
            invalid.hotkey
        )
        .is_err()
    );
}

#[test]
fn payload_bytes_are_upstream_scale_and_ciphertexts_are_randomized()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = WeightsTlockPayload {
        hotkey: vec![7; 32],
        uids: vec![1, 2],
        values: vec![65535, 7],
        version_key: 123,
    };
    assert_eq!(
        WeightsTlockPayload::decode(
            &mut commit_payload(
                payload.hotkey.clone(),
                payload.uids.clone(),
                payload.values.clone(),
                payload.version_key
            )
            .as_slice()
        )?,
        payload
    );
    let now = GENESIS_TIME as f64 + 1000.25;
    let first = generate_commit_v2_at(
        now,
        payload.uids.clone(),
        payload.values.clone(),
        payload.version_key,
        state(),
        1,
        0.25,
        payload.hotkey.clone(),
    )?;
    let second = generate_commit_v2_at(
        now,
        payload.uids.clone(),
        payload.values.clone(),
        payload.version_key,
        state(),
        1,
        0.25,
        payload.hotkey.clone(),
    )?;
    assert_eq!(first.1, second.1);
    assert_ne!(first.0, second.0);
    assert!(!first.0.is_empty());
    Ok(())
}
