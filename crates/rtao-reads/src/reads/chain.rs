// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/chain.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Catalog chain reads.

use crate::ReadError;
use chrono::{DateTime, SecondsFormat, Utc};
use rtao_client::{View, ViewExt};
use serde::Serialize;
use std::time::{Duration, SystemTime};

/// Current timestamp and the block at which it was read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Timestamp {
    /// Pinned chain height.
    pub block: u64,
    /// UTC seconds since the Unix epoch, retaining subsecond precision.
    pub unix: f64,
    /// Python-compatible UTC ISO timestamp.
    pub iso: String,
}

/// Seconds per block from the selected runtime's Aura constant.
// py: sdk/python/bittensor/reads/chain.py:18-22
pub async fn block_time<V: View + ?Sized>(view: &V) -> Result<f64, ReadError> {
    Ok(duration_seconds(view.block_time().await?))
}

pub(crate) fn duration_seconds(duration: Duration) -> f64 {
    // The pinned transport divides by float 1000.0, converting its integer
    // numerator to binary64 before division.
    duration.as_millis() as f64 / 1000.0
}

fn timestamp_seconds(duration: Duration) -> Result<f64, ReadError> {
    let millis = i128::try_from(duration.as_millis()).map_err(|_| ReadError::SurfaceChanged {
        descriptor: "milliseconds",
        detail: "duration exceeds integer range".into(),
    })?;
    rtao_primitives::ratio::integer_division(millis, 1000).ok_or_else(|| {
        ReadError::SurfaceChanged {
            descriptor: "milliseconds",
            detail: "duration conversion failed".into(),
        }
    })
}

/// Current chain timestamp with Python's float and UTC ISO projections.
// py: sdk/python/bittensor/reads/chain.py:11-15
pub async fn timestamp<V: View + ?Sized>(view: &V) -> Result<Timestamp, ReadError> {
    let snapshot = view.pin().await?;
    let duration = snapshot
        .timestamp()
        .await?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| ReadError::SurfaceChanged {
            descriptor: "Timestamp.Now",
            detail: error.to_string(),
        })?;
    // CPython first rounds the millisecond quotient to f64, then rounds the
    // fractional second to microseconds with ties to even. At distant dates
    // this differs from formatting the original exact millisecond integer.
    let floating = timestamp_seconds(duration)?;
    let mut whole = floating.floor() as i64;
    let mut micros = ((floating - floating.floor()) * 1_000_000.0).round_ties_even() as u32;
    if micros == 1_000_000 {
        whole += 1;
        micros = 0;
    }
    let stamp = DateTime::<Utc>::from_timestamp(whole, micros * 1000)
        .filter(|stamp| (1..=9999).contains(&chrono::Datelike::year(stamp)))
        .ok_or_else(|| ReadError::SurfaceChanged {
            descriptor: "Timestamp.Now",
            detail: "timestamp exceeds Python datetime range".into(),
        })?;
    let precision = if stamp.timestamp_subsec_micros() == 0 {
        SecondsFormat::Secs
    } else {
        SecondsFormat::Micros
    };
    Ok(Timestamp {
        block: snapshot.block().number,
        unix: rtao_primitives::ratio::integer_division(
            i128::from(whole) * 1_000_000 + i128::from(micros),
            1_000_000,
        )
        .ok_or_else(|| ReadError::SurfaceChanged {
            descriptor: "Timestamp.Now",
            detail: "timestamp conversion failed".into(),
        })?,
        iso: stamp.to_rfc3339_opts(precision, false),
    })
}
