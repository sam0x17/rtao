// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/timelock/mod.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! UPSTREAM-TARGET: timelock::generate_commit_v2_at (PR-U5)
//! UPSTREAM-PROBE: symbol timelock::generate_commit_v2_at
//! UPSTREAM-FEATURES: none
//!
//! Weight commitment payloads and drand rounds with an explicit clock.

use bittensor_core::{
    CoreError,
    timelock::{
        WeightsTlockPayload,
        constants::{DRAND_PERIOD, GENESIS_TIME, SECURITY_BLOCK_OFFSET},
        encrypt_and_compress,
    },
};
use parity_scale_codec::Encode;

use crate::epoch_schedule::{EpochScheduleState, predict_first_reveal_block};

/// Encode the core's canonical timelocked weight payload without duplicating its format.
pub fn commit_payload(
    hotkey: Vec<u8>,
    uids: Vec<u16>,
    values: Vec<u16>,
    version_key: u64,
) -> Vec<u8> {
    WeightsTlockPayload {
        hotkey,
        uids,
        values,
        version_key,
    }
    .encode()
}

/// Calculate the core-compatible drand round using an injected Unix clock.
///
/// Schedule prediction uses the pallet safety net and commit/reveal processing
/// phases. The cryptographic target retains the core's three-block offset.
#[allow(clippy::arithmetic_side_effects)] // Finite float math; checked integer positions.
pub fn reveal_round_at(
    now: f64,
    state: &EpochScheduleState,
    reveal_period_epochs: u64,
    block_time: f64,
) -> Result<u64, CoreError> {
    if !now.is_finite() || now < 0.0 || !block_time.is_finite() || block_time <= 0.0 {
        return Err(CoreError::Crypto("timelock clock and block time must be finite and non-negative; block time must be positive".into()));
    }
    let first_reveal = predict_first_reveal_block(state, reveal_period_epochs)
        .map_err(|error| CoreError::Crypto(error.to_string()))?;
    let ingest = first_reveal
        .checked_add(SECURITY_BLOCK_OFFSET)
        .ok_or_else(|| CoreError::Crypto("target ingestion block exceeds u64".into()))?;
    let until_ingest = ingest.saturating_sub(state.current_block);
    let target = now + until_ingest as f64 * block_time;
    if !target.is_finite() {
        return Err(CoreError::Crypto(
            "timelock target timestamp overflow".into(),
        ));
    }
    let round = ((target - GENESIS_TIME as f64) / DRAND_PERIOD as f64).floor();
    if round >= u64::MAX as f64 {
        return Err(CoreError::Crypto(
            "timelock reveal round exceeds u64".into(),
        ));
    }
    Ok((round as u64).max(1))
}

/// Encrypt a canonical weight payload at the round calculated from `now`.
///
/// This performs CPU work and no I/O. Async callers run it through their
/// blocking-work adapter.
#[allow(clippy::too_many_arguments)]
pub fn generate_commit_v2_at(
    now: f64,
    uids: Vec<u16>,
    values: Vec<u16>,
    version_key: u64,
    state: EpochScheduleState,
    reveal_period_epochs: u64,
    block_time: f64,
    hotkey: Vec<u8>,
) -> Result<(Vec<u8>, u64), CoreError> {
    if uids.len() != values.len() {
        return Err(CoreError::Crypto(
            "uids and values must have equal lengths".into(),
        ));
    }
    let round = reveal_round_at(now, &state, reveal_period_epochs, block_time)?;
    let payload = commit_payload(hotkey, uids, values, version_key);
    Ok((encrypt_and_compress(&payload, round)?, round))
}

#[cfg(test)]
mod tests {
    use super::*;
    use parity_scale_codec::Decode;

    fn state() -> EpochScheduleState {
        EpochScheduleState {
            last_epoch_block: 10,
            pending_epoch_at: 0,
            subnet_epoch_index: 0,
            tempo: 50,
            blocks_since_last_step: 0,
            current_block: 10,
        }
    }

    #[test]
    fn clock_and_security_offset_pin_round() -> Result<(), CoreError> {
        let now = GENESIS_TIME as f64 + 100.0;
        // First reveal 60; ingestion 63; head 10. 100 + 53 * 12 = 736 seconds.
        assert_eq!(reveal_round_at(now, &state(), 1, 12.0)?, 245);
        assert_eq!(reveal_round_at(0.0, &state(), 1, 12.0)?, 1);
        assert_eq!(reveal_round_at(now + 3.0, &state(), 1, 12.0)?, 246);
        Ok(())
    }

    #[test]
    fn payload_uses_upstream_scale_definition() -> Result<(), parity_scale_codec::Error> {
        let encoded = commit_payload(vec![7; 32], vec![1, 2], vec![65535, 1], 42);
        let mut bytes = encoded.as_slice();
        let decoded = WeightsTlockPayload::decode(&mut bytes)?;
        assert!(bytes.is_empty());
        assert_eq!(decoded.hotkey, vec![7; 32]);
        assert_eq!(decoded.uids, vec![1, 2]);
        assert_eq!(decoded.values, vec![65535, 1]);
        assert_eq!(decoded.version_key, 42);
        Ok(())
    }

    #[test]
    fn invalid_clock_and_timestamp_overflow_fail_before_crypto() {
        for (now, block_time) in [
            (f64::NAN, 12.0),
            (f64::INFINITY, 12.0),
            (-1.0, 12.0),
            (1.0, 0.0),
            (1.0, -1.0),
            (1.0, f64::NAN),
            (f64::MAX, f64::MAX),
        ] {
            assert!(reveal_round_at(now, &state(), 1, block_time).is_err());
        }
    }
}
