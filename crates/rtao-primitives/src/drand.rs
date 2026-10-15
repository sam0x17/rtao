// Derived from RaoFoundation/subtensor sdk/python/bittensor/timelock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// ISO component parsing is adapted from CPython v3.12.12 Modules/_datetimemodule.c (PSF-2.0),
// Copyright (c) 2001-2023 Python Software Foundation; All Rights Reserved.
// Modified by rtao: Rust input handling, chrono calendar checks and exact Unix-time conversion.
// License: LICENSE-PSF in this crate.
//! Local Quicknet beacon arithmetic and Python-compatible time inputs.
//!
//! These helpers perform no I/O. Beacon constants come from the maintained
//! core seam, and explicit timestamps allow deterministic clock injection.

use std::{
    borrow::Cow,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, Utc, Weekday};
use num_bigint::BigUint;
use num_traits::{FromPrimitive, ToPrimitive};
use rtao_core::timelock::constants::{DRAND_PERIOD, GENESIS_TIME};

/// A numeric, textual or typed duration.
#[derive(Clone, Debug, PartialEq)]
pub enum DurationInput<'a> {
    /// Seconds, retaining Python's float semantics and signed durations.
    Seconds(f64),
    /// Python's unit grammar or a bare numeric string.
    Text(Cow<'a, str>),
    /// A non-negative native duration.
    Duration(Duration),
    /// A signed duration with microsecond precision, like Python timedelta.
    Signed(TimeDelta),
}

/// An absolute timestamp, datetime or ISO-8601 moment.
#[derive(Clone, Debug, PartialEq)]
pub enum MomentInput<'a> {
    /// Unix seconds, including a fractional part.
    Timestamp(f64),
    /// ISO-8601 text; a missing offset means UTC.
    Text(Cow<'a, str>),
    /// A system clock reading.
    SystemTime(SystemTime),
    /// An explicitly UTC datetime.
    Utc(DateTime<Utc>),
    /// A naive datetime, interpreted as UTC.
    Naive(NaiveDateTime),
}

macro_rules! seconds_input {
    ($($type:ty),+ $(,)?) => {$ (
        impl From<$type> for DurationInput<'_> {
            fn from(value: $type) -> Self { Self::Seconds(value as f64) }
        }
        impl From<$type> for MomentInput<'_> {
            fn from(value: $type) -> Self { Self::Timestamp(value as f64) }
        }
    )+};
}
seconds_input!(
    f64, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize
);

impl From<bool> for DurationInput<'_> {
    fn from(value: bool) -> Self {
        Self::Seconds(f64::from(u8::from(value)))
    }
}
impl From<bool> for MomentInput<'_> {
    fn from(value: bool) -> Self {
        Self::Timestamp(f64::from(u8::from(value)))
    }
}

macro_rules! text_input {
    ($type:ident) => {
        impl<'a> From<&'a str> for $type<'a> {
            fn from(value: &'a str) -> Self {
                Self::Text(Cow::Borrowed(value))
            }
        }
        impl From<String> for $type<'_> {
            fn from(value: String) -> Self {
                Self::Text(Cow::Owned(value))
            }
        }
    };
}
text_input!(DurationInput);
text_input!(MomentInput);

impl From<Duration> for DurationInput<'_> {
    fn from(value: Duration) -> Self {
        Self::Duration(value)
    }
}
impl From<TimeDelta> for DurationInput<'_> {
    fn from(value: TimeDelta) -> Self {
        Self::Signed(value)
    }
}
impl From<SystemTime> for MomentInput<'_> {
    fn from(value: SystemTime) -> Self {
        Self::SystemTime(value)
    }
}
impl<Tz: chrono::TimeZone> From<DateTime<Tz>> for MomentInput<'_> {
    fn from(value: DateTime<Tz>) -> Self {
        Self::Utc(value.with_timezone(&Utc))
    }
}
impl From<NaiveDateTime> for MomentInput<'_> {
    fn from(value: NaiveDateTime) -> Self {
        Self::Naive(value)
    }
}

/// A malformed time input or a round outside the native chain range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelockError(String);

impl TimelockError {
    /// Diagnostic text, matching Python for malformed duration/moment inputs.
    pub fn message(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for TimelockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for TimelockError {}

// py: sdk/python/bittensor/timelock.py:110-130 (parse_duration)
/// Parse seconds or composable `w`, `d`, `h`, `m`, `s` duration units.
///
/// Bare numeric strings use Python's float grammar. Units may repeat or occur
/// in any order, with unsigned numbers and optional whitespace before units.
/// This parser retains negative and non-finite numeric values; callers decide
/// whether a particular reveal operation permits them.
#[doc(alias = "bittensor.timelock.parse_duration")]
pub fn parse_duration<'a>(value: impl Into<DurationInput<'a>>) -> Result<f64, TimelockError> {
    match value.into() {
        DurationInput::Seconds(value) => Ok(value),
        DurationInput::Duration(value) => Ok(value.as_secs_f64()),
        DurationInput::Signed(value) => {
            let micros =
                i128::from(value.num_seconds()) * 1_000_000 + i128::from(value.subsec_micros());
            Ok(crate::python::int_div_to_f64(micros, 1_000_000))
        }
        DurationInput::Text(value) => {
            let text = value.trim_matches(crate::python::whitespace);
            if let Some(number) = crate::python::float(text) {
                return Ok(number);
            }
            parse_duration_units(text).ok_or_else(|| TimelockError(format!(
                "cannot parse duration {}; use seconds or units like '90s', '15m', '1h30m', '2d'",
                crate::python::string_repr(&value)
            )))
        }
    }
}

fn parse_duration_units(text: &str) -> Option<f64> {
    let chars: Vec<_> = text.chars().collect();
    let mut index = 0;
    let mut total = 0.0;
    let mut count = 0;
    while index < chars.len() {
        while index < chars.len() && crate::python::whitespace(chars[index]) {
            index += 1;
        }
        if index == chars.len() {
            break;
        }
        let mut number = String::new();
        while index < chars.len() {
            let Some(digit) = crate::python::decimal_digit(chars[index]) else {
                break;
            };
            number.push(char::from(b'0' + digit));
            index += 1;
        }
        if number.is_empty() {
            return None;
        }
        if chars.get(index) == Some(&'.') {
            index += 1;
            number.push('.');
            let start = index;
            while index < chars.len() {
                let Some(digit) = crate::python::decimal_digit(chars[index]) else {
                    break;
                };
                number.push(char::from(b'0' + digit));
                index += 1;
            }
            if index == start {
                return None;
            }
        }
        while index < chars.len() && crate::python::whitespace(chars[index]) {
            index += 1;
        }
        let unit = chars.get(index)?.to_ascii_lowercase();
        let seconds = match unit {
            'w' => 604800.0,
            'd' => 86400.0,
            'h' => 3600.0,
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        total += crate::python::float(&number)? * seconds;
        count += 1;
        index += 1;
    }
    (count > 0).then_some(total)
}

// py: sdk/python/bittensor/timelock.py:90-102 (format_duration)
/// Render a duration as whole `d h m s` units, truncating fractional seconds.
///
/// Non-positive values render as `"0s"`; a week renders as seven days.
#[doc(alias = "bittensor.timelock.format_duration")]
pub fn format_duration<'a>(value: impl Into<DurationInput<'a>>) -> Result<String, TimelockError> {
    let seconds = parse_duration(value)?;
    finite_integer(seconds)?;
    if seconds <= 0.0 {
        return Ok("0s".into());
    }
    let mut remaining = BigUint::from_f64(seconds.trunc()).ok_or_else(round_range)?;
    let mut parts = Vec::new();
    for (unit, size) in [('d', 86400u64), ('h', 3600), ('m', 60), ('s', 1)] {
        let amount = &remaining / size;
        remaining %= size;
        if amount != BigUint::from(0u8) {
            parts.push(format!("{amount}{unit}"));
        }
    }
    Ok(if parts.is_empty() {
        "0s".into()
    } else {
        parts.join(" ")
    })
}

// py: sdk/python/bittensor/timelock.py:133-153 (_parse_when)
/// Convert an absolute moment to Unix seconds; naive values mean UTC.
///
/// Strings follow Python's ISO grammar, including basic and ISO-week dates,
/// comma fractions, arbitrary single separators and timezone offsets. Numeric
/// strings are not timestamps. Fractional text precision truncates to micros.
pub fn parse_when<'a>(value: impl Into<MomentInput<'a>>) -> Result<f64, TimelockError> {
    match value.into() {
        MomentInput::Timestamp(value) => Ok(value),
        MomentInput::SystemTime(value) => Ok(system_timestamp(value)),
        MomentInput::Utc(value) => Ok(datetime_timestamp(value.naive_utc())),
        MomentInput::Naive(value) => Ok(datetime_timestamp(value)),
        MomentInput::Text(value) => parse_iso_moment(value.trim_matches(crate::python::whitespace)).ok_or_else(|| TimelockError(format!(
            "cannot parse moment {}; use ISO-8601 like '2026-07-08T12:00Z' (no offset means UTC) or a unix timestamp",
            crate::python::string_repr(&value)
        ))),
    }
}

fn system_timestamp(value: SystemTime) -> f64 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(value) => value.as_secs_f64(),
        Err(value) => -value.duration().as_secs_f64(),
    }
}

fn datetime_timestamp(value: NaiveDateTime) -> f64 {
    let utc = value.and_utc();
    let micros =
        i128::from(utc.timestamp()) * 1_000_000 + i128::from(utc.timestamp_subsec_micros());
    crate::python::int_div_to_f64(micros, 1_000_000)
}

// Python's fromisoformat extends RFC3339. Parse its date/time component grammar
// explicitly, leaving calendar validation to chrono; no local timezone lookup.
fn parse_iso_moment(text: &str) -> Option<f64> {
    let replaced = text.replace('Z', "+00:00");
    let text = replaced.as_str();
    let bytes = text.as_bytes();
    let split = date_split(bytes)?;
    let date = iso_date(text.get(..split)?)?;
    let mut micros = i128::from(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp()) * 1_000_000;
    if split < text.len() {
        let rest = text.get(split..)?;
        let separator = rest.chars().next()?;
        let time = rest.get(separator.len_utf8()..)?;
        let zone_at = time.find(['+', '-']);
        let clock = zone_at.map_or(time, |index| &time[..index]);
        let (hour, minute, second, fraction) = iso_clock(clock)?;
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        micros += i128::from(hour * 3600 + minute * 60 + second) * 1_000_000 + i128::from(fraction);
        if let Some(index) = zone_at {
            let zone = &time[index..];
            let sign = if zone.starts_with('-') { -1i128 } else { 1 };
            let (hour, minute, second, fraction) = iso_clock(&zone[1..])?;
            let whole = i128::from(hour * 3600 + minute * 60 + second);
            // Python treats an offset whose whole seconds are zero as UTC,
            // even when its textual offset includes fractional microseconds.
            let offset = if whole == 0 {
                0
            } else {
                sign * (whole * 1_000_000 + i128::from(fraction))
            };
            if offset.abs() >= 86_400_000_000 {
                return None;
            }
            micros -= offset;
        }
    }
    Some(crate::python::int_div_to_f64(micros, 1_000_000))
}

fn date_split(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 7 {
        return None;
    }
    if bytes.len() == 7 {
        return Some(7);
    }
    if bytes[4] == b'-' {
        if bytes[5] != b'W' {
            return Some(10);
        }
        if bytes.len() > 8 && bytes[8] == b'-' {
            if bytes.len() == 9 {
                return None;
            }
            if bytes.len() > 10 && bytes[10].is_ascii_digit() {
                return Some(8);
            }
            return Some(10);
        }
        Some(8)
    } else if bytes[4] == b'W' {
        let index = (7..bytes.len())
            .find(|index| !bytes[*index].is_ascii_digit())
            .unwrap_or(bytes.len());
        Some(if index < 9 {
            index
        } else if index % 2 == 0 {
            7
        } else {
            8
        })
    } else {
        Some(8)
    }
}

fn digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        bytes
            .iter()
            .fold(0, |number, digit| number * 10 + u32::from(digit - b'0')),
    )
}

fn iso_date(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    let year = i32::try_from(digits(bytes.get(..4)?)?).ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let separators = bytes.get(4) == Some(&b'-');
    let start = if separators { 5 } else { 4 };
    if bytes.get(start) == Some(&b'W') {
        let week = digits(bytes.get(start + 1..start + 3)?)?;
        let day_start = start + 3;
        let day = if bytes.len() == day_start {
            1
        } else {
            let position = if separators {
                if bytes.get(day_start) != Some(&b'-') {
                    return None;
                }
                day_start + 1
            } else {
                day_start
            };
            if bytes.len() != position + 1 {
                return None;
            }
            digits(bytes.get(position..position + 1)?)?
        };
        let weekday = match day {
            1 => Weekday::Mon,
            2 => Weekday::Tue,
            3 => Weekday::Wed,
            4 => Weekday::Thu,
            5 => Weekday::Fri,
            6 => Weekday::Sat,
            7 => Weekday::Sun,
            _ => return None,
        };
        let date = NaiveDate::from_isoywd_opt(year, week, weekday)?;
        (1..=9999).contains(&date.year()).then_some(date)
    } else {
        let month = digits(bytes.get(start..start + 2)?)?;
        let day_start = if separators {
            if bytes.get(start + 2) != Some(&b'-') {
                return None;
            }
            start + 3
        } else {
            start + 2
        };
        if bytes.len() != day_start + 2 {
            return None;
        }
        NaiveDate::from_ymd_opt(year, month, digits(bytes.get(day_start..day_start + 2)?)?)
    }
}

fn iso_clock(text: &str) -> Option<(u32, u32, u32, u32)> {
    let bytes = text.as_bytes();
    let mut parts = [0u32; 3];
    let mut index = 0;
    let mut separated = false;
    for (position, part) in parts.iter_mut().enumerate() {
        *part = digits(bytes.get(index..index + 2)?)?;
        index += 2;
        if index == bytes.len() {
            return Some((parts[0], parts[1], parts[2], 0));
        }
        let marker = *bytes.get(index)?;
        index += 1;
        if position == 0 {
            separated = marker == b':';
        }
        if index >= bytes.len() {
            return None;
        }
        if separated && marker == b':' {
            continue;
        }
        if matches!(marker, b'.' | b',') {
            break;
        }
        if !separated {
            index -= 1;
        } else {
            return None;
        }
    }
    let fraction = bytes.get(index..)?;
    if fraction.is_empty() || !fraction.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let count = fraction.len().min(6);
    let micros = digits(&fraction[..count])? * 10u32.pow(6 - count as u32);
    Some((parts[0], parts[1], parts[2], micros))
}

fn finite_integer(value: f64) -> Result<(), TimelockError> {
    if value.is_nan() {
        Err(TimelockError("cannot convert float NaN to integer".into()))
    } else if value.is_infinite() {
        Err(TimelockError(
            "cannot convert float infinity to integer".into(),
        ))
    } else {
        Ok(())
    }
}
fn round_range() -> TimelockError {
    TimelockError("beacon round is outside the native u64 range".into())
}

// py: sdk/python/bittensor/timelock.py:156-159 (current_round)
/// Latest published round at an injected Unix time, or the current clock.
///
/// Times before genesis clamp to round one. Non-finite timestamps and rounds
/// outside the native `u64` chain range return an error.
#[doc(alias = "bittensor.timelock.current_round")]
pub fn current_round(at: Option<f64>) -> Result<u64, TimelockError> {
    let now = at.unwrap_or_else(|| system_timestamp(SystemTime::now()));
    let elapsed = now - GENESIS_TIME as f64;
    // Match CPython's float floor division, including quotient correction.
    let remainder = elapsed % DRAND_PERIOD as f64;
    let mut quotient = (elapsed - remainder) / DRAND_PERIOD as f64;
    if remainder != 0.0 && remainder.is_sign_negative() {
        quotient -= 1.0;
    }
    finite_integer(quotient)?;
    let floor = quotient.floor();
    let floor = if quotient - floor > 0.5 {
        floor + 1.0
    } else {
        floor
    };
    if floor < 0.0 {
        return Ok(1);
    }
    BigUint::from_f64(floor)
        .and_then(|round| (round + 1u8).to_u64())
        .ok_or_else(round_range)
}

// py: sdk/python/bittensor/timelock.py:162-174 (round_at)
/// First reveal round for a moment, using Python's truncated milliseconds.
///
/// This retains the Python helper's submillisecond behavior: a timestamp less
/// than one millisecond after a beacon pulse still selects that pulse.
#[doc(alias = "bittensor.timelock.round_at")]
pub fn round_at<'a>(when: impl Into<MomentInput<'a>>) -> Result<u64, TimelockError> {
    let elapsed = parse_when(when)? - GENESIS_TIME as f64;
    if elapsed <= 0.0 {
        return Ok(1);
    }
    let milliseconds = elapsed * 1000.0;
    finite_integer(milliseconds)?;
    let milliseconds = BigUint::from_f64(milliseconds.trunc()).ok_or_else(round_range)?;
    let period = DRAND_PERIOD * 1000;
    ((milliseconds + period - 1u8) / period + 1u8)
        .to_u64()
        .ok_or_else(round_range)
}

// py: sdk/python/bittensor/timelock.py:177-180 (reveal_time)
/// UTC publication time of a native chain round.
///
/// Round zero retains Python's meaning, one beacon period before genesis.
/// Results must fit Python's datetime year range, `1..=9999`.
#[doc(alias = "bittensor.timelock.reveal_time")]
pub fn reveal_time(reveal_round: u64) -> Result<DateTime<Utc>, TimelockError> {
    let timestamp =
        i128::from(GENESIS_TIME) + (i128::from(reveal_round) - 1) * i128::from(DRAND_PERIOD);
    let seconds = i64::try_from(timestamp)
        .map_err(|_| TimelockError("timestamp out of range for platform time_t".into()))?;
    let moment = DateTime::from_timestamp(seconds, 0)
        .ok_or_else(|| TimelockError("year is out of range".into()))?;
    if !(1..=9999).contains(&moment.year()) {
        return Err(TimelockError(format!(
            "year {} is out of range",
            moment.year()
        )));
    }
    Ok(moment)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn duration_grammar_and_order() -> Result<(), TimelockError> {
        for (value, expected) in [
            ("1h30m", 5400.0),
            ("30m1h", 5400.0),
            ("1w 2D 3h 4M 5.5s", 788645.5),
            ("1m2m", 180.0),
            ("١.٥ H", 5400.0),
            ("1e3", 1000.0),
            ("1_000", 1000.0),
            ("-1", -1.0),
            ("  +.5  ", 0.5),
        ] {
            assert_eq!(parse_duration(value)?, expected, "{value}");
        }
        assert!(parse_duration("NaN")?.is_nan());
        assert_eq!(parse_duration("-inf")?, f64::NEG_INFINITY);
        assert_eq!(parse_duration(true)?, 1.0);
        assert_eq!(parse_duration(false)?, 0.0);
        assert_eq!(parse_when(true)?, 1.0);
        for value in [
            ".5s", "1.s", "1e3s", "-1s", "1_000s", "1 2s", "h", "1s junk", "",
        ] {
            assert_eq!(
                parse_duration(value).unwrap_err().message(),
                format!(
                    "cannot parse duration {}; use seconds or units like '90s', '15m', '1h30m', '2d'",
                    crate::python::string_repr(value)
                )
            );
        }
        Ok(())
    }

    #[test]
    fn duration_formatting_and_signed_timedelta() -> Result<(), TimelockError> {
        assert_eq!(format_duration(0.9)?, "0s");
        assert_eq!(format_duration(-1.5)?, "0s");
        assert_eq!(format_duration(Duration::from_secs(788645))?, "9d 3h 4m 5s");
        assert_eq!(format_duration(3600.999)?, "1h");
        assert_eq!(parse_duration(TimeDelta::microseconds(-1))?, -0.000001);
        assert_eq!(
            parse_duration(TimeDelta::microseconds(-1_000_001))?,
            -1.000001
        );
        assert_eq!(
            format_duration(f64::NAN).unwrap_err().message(),
            "cannot convert float NaN to integer"
        );
        Ok(())
    }

    #[test]
    fn rounds_before_at_and_after_slots() -> Result<(), TimelockError> {
        let genesis = GENESIS_TIME as f64;
        assert_eq!(current_round(Some(genesis - 100.0))?, 1);
        assert_eq!(current_round(Some(genesis))?, 1);
        assert_eq!(current_round(Some(genesis + 2.999))?, 1);
        assert_eq!(current_round(Some(genesis + 3.0))?, 2);
        assert_eq!(current_round(Some(genesis + 3.001))?, 2);
        assert_eq!(round_at(genesis - 100.0)?, 1);
        assert_eq!(round_at(genesis)?, 1);
        assert_eq!(round_at(genesis + 0.0005)?, 1);
        assert_eq!(round_at(genesis + 0.002)?, 2);
        assert_eq!(round_at(genesis + 3.0)?, 2);
        assert_eq!(round_at(genesis + 3.0005)?, 2);
        assert_eq!(round_at(genesis + 3.002)?, 3);
        assert_eq!(
            reveal_time(0)?.timestamp(),
            GENESIS_TIME as i64 - DRAND_PERIOD as i64
        );
        assert_eq!(reveal_time(1)?.timestamp(), GENESIS_TIME as i64);
        assert_eq!(
            reveal_time(10)?.timestamp(),
            GENESIS_TIME as i64 + 9 * DRAND_PERIOD as i64
        );
        Ok(())
    }

    #[test]
    fn iso_strings_normalize_to_utc() -> Result<(), TimelockError> {
        let midnight = parse_when("2026-07-08")?;
        for value in [
            "2026-07-08T00:00Z",
            "20260708T000000",
            "2026-07-08 00",
            "2026-07-08🐱00:00:00",
            "2026-07-08T02:30:00+02:30",
            "2026-07-07T23:00:00-01:00",
            "2026-W28-3T00:00",
            "2026W283T0000",
            "2026-07-08T00:00:00+00:00:00.5",
        ] {
            assert_eq!(parse_when(value)?.to_bits(), midnight.to_bits(), "{value}");
        }
        assert_eq!(
            parse_when("2026-07-08T12:00:00,123456789")?,
            parse_when("2026-07-08T12:00:00.123456")?
        );
        assert_eq!(
            parse_when("2026-07-08T12.5")?,
            parse_when("2026-07-08T12:00:00.5")?
        );
        assert_eq!(parse_when("2026-W28")?, parse_when("2026-07-06")?);
        assert_eq!(
            parse_when("2026-07-08T01:00:00+00:59:59.5")?,
            parse_when("2026-07-08T00:00:00.5")?
        );
        let naive = NaiveDate::from_ymd_opt(2026, 7, 8)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(parse_when(naive)?, midnight);
        assert_eq!(parse_when(naive.and_utc())?, midnight);
        let offset = DateTime::parse_from_rfc3339("2026-07-08T02:30:00+02:30")
            .map_err(|error| TimelockError(error.to_string()))?;
        assert_eq!(parse_when(offset)?, midnight);
        Ok(())
    }

    #[test]
    fn malformed_moments_and_range_errors() {
        for value in [
            "1692803367",
            "2026-07-08T12:00z",
            "2026-02-30",
            "2026-07-08T24:00",
            "2026-07-08T12:00:60",
            "2026-07-08T12:00+24:00",
            "0000-01-01",
            "2026-07-08T",
            "2026-07-08TT12:00",
        ] {
            assert_eq!(
                parse_when(value).unwrap_err().message(),
                format!(
                    "cannot parse moment {}; use ISO-8601 like '2026-07-08T12:00Z' (no offset means UTC) or a unix timestamp",
                    crate::python::string_repr(value)
                )
            );
        }
        assert!(current_round(Some(f64::NAN)).is_err());
        assert!(round_at(f64::INFINITY).is_err());
        assert!(current_round(Some(f64::MAX)).is_err());
        assert!(round_at(f64::MAX).is_err());
        assert!(reveal_time(u64::MAX).is_err());
    }
}
