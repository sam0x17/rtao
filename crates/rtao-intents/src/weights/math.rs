// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.

use std::{cmp::Ordering, fmt};

use rtao_catalog::ErrorCode;
use rtao_primitives::{NetUid, Uid};

const U16_MAX: f64 = u16::MAX as f64;

/// Chain parameters used by Python's pure `_conform` helper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConformState {
    /// UID of the signing validator, for the single self-weight exception.
    pub uid: Uid,
    /// Required number of nonzero weights after quantization.
    pub min_allowed_weights: u16,
    /// Maximum weight share, expressed as a fraction of 65535.
    pub max_weight_limit: u16,
}

/// Validation or minimum-count failure while preparing a weight row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightsError {
    message: String,
    code: Option<ErrorCode>,
}

impl WeightsError {
    fn validation(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
        }
    }

    /// Python's semantic chain error code, when its helper raises `ChainError`.
    pub const fn code(&self) -> Option<ErrorCode> {
        self.code
    }
}

impl fmt::Display for WeightsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WeightsError {}

/// Max-upscale and ties-to-even quantize weights to u16, dropping zero values.
///
/// Empty and all-zero inputs return an empty row. UID width is enforced by
/// [`Uid`]; weights must be finite and non-negative.
#[doc(alias = "bittensor.intents.weights.normalize")]
pub fn normalize(uids: &[Uid], weights: &[f64]) -> Result<(Vec<Uid>, Vec<u16>), WeightsError> {
    if uids.len() != weights.len() {
        return Err(WeightsError::validation(format!(
            "uids and weights must be equal length: {} vs {}.",
            uids.len(),
            weights.len()
        )));
    }
    if weights
        .iter()
        .any(|weight| !weight.is_finite() || *weight < 0.0)
    {
        return Err(WeightsError::validation(
            "Weights must be finite and non-negative.",
        ));
    }
    let top = weights.iter().copied().fold(0.0, f64::max);
    if top == 0.0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut out_uids = Vec::with_capacity(uids.len());
    let mut out_values = Vec::with_capacity(uids.len());
    for (&uid, &weight) in uids.iter().zip(weights) {
        let value = ((weight / top) * U16_MAX).round_ties_even() as u16;
        if value != 0 {
            out_uids.push(uid);
            out_values.push(value);
        }
    }
    Ok((out_uids, out_values))
}

// CPython 3.12+ sum(float) uses Neumaier compensation; itertools.accumulate
// remains a left fold. Do not replace either operation with the other.
pub(super) fn python_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum: f64 = 0.0;
    let mut correction: f64 = 0.0;
    for value in values {
        let next = sum + value;
        correction += if sum.abs() >= value.abs() {
            (sum - next) + value
        } else {
            (value - next) + sum
        };
        sum = next;
    }
    if correction != 0.0 && correction.is_finite() {
        sum += correction;
    }
    sum
}

/// Sum-normalize a row and clip its largest normalized share to `limit`.
///
/// An impossible limit (`len * limit <= 1`) produces a uniform row, preserving
/// Python's branch order. This helper retains the arithmetic of the Python
/// helper; [`normalize`] validates the resulting weights before encoding.
#[doc(alias = "bittensor.intents.weights.clip_to_max_weight")]
pub fn clip_to_max_weight(weights: &[f64], limit: f64) -> Result<Vec<f64>, WeightsError> {
    let n = weights.len();
    let total = python_sum(weights.iter().copied());
    if n == 0 || total <= 0.0 {
        return Err(WeightsError::validation(
            "All weights are zero; nothing to set.",
        ));
    }
    if n as f64 * limit <= 1.0 {
        return Ok(vec![1.0 / n as f64; n]);
    }
    let mut estimation: Vec<_> = weights.iter().map(|weight| weight / total).collect();
    estimation.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let Some(&largest) = estimation.last() else {
        return Err(WeightsError::validation(
            "All weights are zero; nothing to set.",
        ));
    };
    if largest <= limit {
        return Ok(weights.iter().map(|weight| weight / total).collect());
    }
    let epsilon = 1e-7;
    let mut cumulative = Vec::with_capacity(n);
    let mut partial = 0.0;
    let mut n_values = 0;
    for (index, &value) in estimation.iter().enumerate() {
        partial += value;
        cumulative.push(partial);
        if value / ((n - index - 1) as f64 * value + partial + epsilon) < limit {
            n_values += 1;
        }
    }
    // Python's cumsum[n_values - 1] selects the last value when n_values is 0.
    let cutoff_index = if n_values == 0 { n - 1 } else { n_values - 1 };
    let scale =
        (limit * cumulative[cutoff_index] - epsilon) / (1.0 - limit * (n - n_values) as f64);
    let cutoff = scale * total;
    // Python min(weight, cutoff) preserves weight when comparison is unordered.
    let clipped: Vec<_> = weights
        .iter()
        .map(|&weight| if cutoff < weight { cutoff } else { weight })
        .collect();
    let clipped_total = python_sum(clipped.iter().copied());
    Ok(clipped
        .iter()
        .map(|weight| weight / clipped_total)
        .collect())
}

/// Fit a row to the subnet's limits and return the exact u16 submission row.
///
/// A single self-weight is exempt from the share and minimum-count limits.
/// `raw_u16` retains integer ratios and checks them without clipping.
#[doc(alias = "bittensor.intents.weights._conform")]
pub fn conform(
    uids: &[Uid],
    weights: &[f64],
    state: ConformState,
    netuid: NetUid,
    raw_u16: bool,
) -> Result<(Vec<Uid>, Vec<u16>), WeightsError> {
    let is_self_weight = uids == [state.uid];
    let row = if raw_u16 {
        if uids.len() != weights.len() {
            return Err(WeightsError::validation(
                "Raw weights require equal-length lists and u16 UIDs.",
            ));
        }
        if weights.iter().any(|weight| {
            !weight.is_finite() || *weight < 0.0 || *weight > U16_MAX || weight.trunc() != *weight
        }) {
            return Err(WeightsError::validation(
                "Raw weights must be integers in 0..65535.",
            ));
        }
        let mut row = (
            Vec::with_capacity(uids.len()),
            Vec::with_capacity(uids.len()),
        );
        for (&uid, &weight) in uids.iter().zip(weights) {
            if weight > 0.0 {
                row.0.push(uid);
                row.1.push(weight as u16);
            }
        }
        let Some(&top) = row.1.iter().max() else {
            return Err(WeightsError::validation(
                "All weights are zero; nothing to set.",
            ));
        };
        let total: u128 = row.1.iter().map(|&weight| u128::from(weight)).sum();
        if !is_self_weight
            && u128::from(top) * u128::from(u16::MAX) > u128::from(state.max_weight_limit) * total
        {
            return Err(WeightsError::validation(
                "Raw weights exceed max_weight_limit; change the integer ratios or subnet limit.",
            ));
        }
        row
    } else if !is_self_weight && state.max_weight_limit < u16::MAX {
        normalize(
            uids,
            &clip_to_max_weight(weights, f64::from(state.max_weight_limit) / U16_MAX)?,
        )?
    } else {
        normalize(uids, weights)?
    };
    if row.0.is_empty() {
        return Err(WeightsError::validation(
            "All weights are zero; nothing to set.",
        ));
    }
    if !is_self_weight && row.0.len() < usize::from(state.min_allowed_weights) {
        return Err(WeightsError {
            message: format!(
                "Subnet {} requires at least {} weights; only {} are nonzero after normalization.",
                netuid.0,
                state.min_allowed_weights,
                row.0.len()
            ),
            code: Some(ErrorCode::InvalidArgument),
        });
    }
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ConformState {
        ConformState {
            uid: Uid(0),
            min_allowed_weights: 1,
            max_weight_limit: u16::MAX,
        }
    }

    #[test]
    fn ties_even_and_small_values() -> Result<(), WeightsError> {
        assert_eq!(
            normalize(&[Uid(1), Uid(2), Uid(3), Uid(4)], &[65535.0, 0.5, 1.5, 2.5])?,
            (vec![Uid(1), Uid(3), Uid(4)], vec![65535, 2, 2])
        );
        assert_eq!(
            normalize(&[Uid(1), Uid(2)], &[3.0, 1.0])?,
            (vec![Uid(1), Uid(2)], vec![65535, 21845])
        );
        assert_eq!(normalize(&[Uid(1)], &[0.0])?, (vec![], vec![]));
        Ok(())
    }

    #[test]
    fn raw_full_row_keeps_exact_small_weights() -> Result<(), WeightsError> {
        let uids: Vec<_> = (0..2500).map(Uid).collect();
        let mut weights = vec![1.0; uids.len()];
        weights[0] = 65534.0;
        let row = conform(&uids, &weights, state(), NetUid(1), true)?;
        assert_eq!(row.0, uids);
        assert_eq!(row.1[0], 65534);
        assert!(row.1[1..].iter().all(|&weight| weight == 1));
        Ok(())
    }

    #[test]
    fn raw_limits_and_invalid_inputs() {
        for bad in [-1.0, 65536.0, 1.5, f64::INFINITY, f64::NAN] {
            assert_eq!(
                conform(&[Uid(1)], &[bad], state(), NetUid(1), true)
                    .err()
                    .map(|error| error.to_string()),
                Some("Raw weights must be integers in 0..65535.".into())
            );
        }
        let limited = ConformState {
            max_weight_limit: 32767,
            ..state()
        };
        assert!(conform(&[Uid(1), Uid(2)], &[100.0, 1.0], limited, NetUid(1), true).is_err());
        assert!(conform(&[Uid(1)], &[0.0], state(), NetUid(1), true).is_err());
        assert!(normalize(&[Uid(1)], &[]).is_err());
        assert!(normalize(&[Uid(1)], &[f64::NAN]).is_err());
    }

    #[test]
    fn count_after_quantization_and_self_weight_exception() -> Result<(), WeightsError> {
        let limited = ConformState {
            min_allowed_weights: 10,
            max_weight_limit: 1,
            ..state()
        };
        assert_eq!(
            conform(&[Uid(0)], &[1.0], limited, NetUid(1), false)?,
            (vec![Uid(0)], vec![65535])
        );
        let err = conform(
            &[Uid(1), Uid(2)],
            &[1.0, 1e-20],
            ConformState {
                min_allowed_weights: 2,
                ..state()
            },
            NetUid(1),
            false,
        )
        .err();
        assert_eq!(
            err.as_ref().and_then(WeightsError::code),
            Some(ErrorCode::InvalidArgument)
        );
        assert_eq!(
            err.map(|error| error.to_string()),
            Some(
                "Subnet 1 requires at least 2 weights; only 1 are nonzero after normalization."
                    .into()
            )
        );
        Ok(())
    }

    #[test]
    fn compensated_sum_and_left_fold_have_distinct_roles() {
        assert_eq!(python_sum([1e16, 1.0, 1.0]), 1e16 + 2.0);
        assert_eq!(python_sum([f64::MAX, f64::MAX]), f64::INFINITY);
        assert_eq!(python_sum([-0.0]).to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn clipping_uniform_and_normalized_branches() -> Result<(), WeightsError> {
        assert_eq!(clip_to_max_weight(&[3.0, 1.0], 0.5)?, vec![0.5, 0.5]);
        assert_eq!(clip_to_max_weight(&[3.0, 1.0], 0.9)?, vec![0.75, 0.25]);
        let values = clip_to_max_weight(&[100.0, 1.0, 1.0], 0.6)?;
        assert!(values[0] <= 0.6);
        assert!((values.iter().sum::<f64>() - 1.0).abs() < 1e-15);
        assert!(clip_to_max_weight(&[], 0.5).is_err());
        assert!(clip_to_max_weight(&[0.0], 0.5).is_err());
        Ok(())
    }
}
